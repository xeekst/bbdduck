//! The sync engine (Node B side): scans the remote share, compares with the
//! local folder (incremental), downloads files with N parallel workers under a
//! shared bandwidth cap, and emits progress events to the frontend.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, RecvTimeoutError};
use rusqlite::{params, Connection, OptionalExtension};
use tauri::{AppHandle, Emitter, Manager, Runtime};

use super::client::{list_remote_files, BandwidthLimiter};
use super::logging;
use super::model::*;
use super::protocol::{
    connect_with_timeout, mtime_secs, read_msg, write_msg, ClientMsg, ServerMsg, PROTOCOL_VERSION,
};
use super::MAX_PATH_FAILURES;

const CHUNK: usize = 256 * 1024;
/// Max retries per file after the first failure.
const MAX_RETRIES: u32 = MAX_PATH_FAILURES;
/// Delay between retry attempts.
const RETRY_INTERVAL: Duration = Duration::from_secs(3);
/// Max listing retries after the scan connection drops mid-way.
const MAX_LIST_RETRIES: u32 = 30;
/// Delay between listing retries.
const LIST_RETRY_INTERVAL: Duration = Duration::from_secs(3);

/// UI receives one bounded active-transfer snapshot at this interval.
const PROGRESS_EMIT_INTERVAL: Duration = Duration::from_millis(500);
/// Number of entries committed to a temporary SQLite manifest at once.
const MANIFEST_BATCH_SIZE: usize = 8192;
pub struct JobProgress {
    pub total_files: u64,
    pub done_files: u64,
    pub failed_files: u64,
    pub skipped_files: u64,
    pub total_bytes: u64,
    pub done_bytes: u64,
    pub speed: u64,
    pub error: Option<String>,
    pub scanned_files: u64,
    pub active_files: u64,
    pub listing_complete: bool,
    pub list_attempt: u32,
    pub phase: String,
    pub activity: String,
    pub current_file: Option<String>,
}

impl Default for JobProgress {
    fn default() -> Self {
        Self {
            total_files: 0,
            done_files: 0,
            failed_files: 0,
            skipped_files: 0,
            total_bytes: 0,
            done_bytes: 0,
            speed: 0,
            error: None,
            scanned_files: 0,
            active_files: 0,
            listing_complete: false,
            list_attempt: 1,
            phase: "preparing".into(),
            activity: "正在初始化同步任务".into(),
            current_file: None,
        }
    }
}

pub struct JobSummary {
    pub status: JobStatus,
    pub error: Option<String>,
    pub message: Option<String>,
}

type ActiveTransfers = Arc<Mutex<HashMap<String, ActiveFileProgress>>>;

struct SyncTask {
    path: String,
    size: u64,
}

/// A file waiting for (or undergoing) a retry attempt.
struct RetryTask {
    path: String,
    size: u64,
    /// Upcoming attempt number (1-based, up to MAX_RETRIES).
    attempt: u32,
    /// Earliest time the next attempt may start.
    next_attempt_at: Instant,
}

/// A shared retry queue with a condition variable. `pop_ready` returns the
/// first task whose backoff has elapsed (not necessarily the head), or waits;
/// returns `None` once the main transfer is done and the queue is empty, or on stop.
struct RetryQueue {
    items: Mutex<VecDeque<RetryTask>>,
    condvar: Condvar,
}

impl RetryQueue {
    fn new() -> Self {
        Self {
            items: Mutex::new(VecDeque::new()),
            condvar: Condvar::new(),
        }
    }

    fn push(&self, task: RetryTask) {
        let mut items = self.items.lock().unwrap();
        items.push_back(task);
        self.condvar.notify_one();
    }

    fn is_empty(&self) -> bool {
        self.items.lock().unwrap().is_empty()
    }

    fn notify_all(&self) {
        self.condvar.notify_all();
    }

    fn pop_ready(&self, stop: &AtomicBool, main_done: &AtomicBool) -> Option<RetryTask> {
        loop {
            let mut items = self.items.lock().unwrap();
            if stop.load(Ordering::Relaxed) {
                return None;
            }
            let now = Instant::now();
            if let Some(idx) = items.iter().position(|t| t.next_attempt_at <= now) {
                return items.remove(idx);
            }
            if items.is_empty() && main_done.load(Ordering::Relaxed) {
                return None;
            }
            let wait = items
                .iter()
                .map(|t| t.next_attempt_at.saturating_duration_since(now))
                .min()
                .map(|d| d.min(Duration::from_millis(250)))
                .unwrap_or(Duration::from_millis(250));
            let guard = match self.condvar.wait_timeout(items, wait) {
                Ok(x) => x.0,
                Err(_) => return None,
            };
            drop(guard); // release before the next iteration re-locks
        }
    }
}

/// Hash a relative path into a u64 before writing it to the disk manifest.
/// A collision only makes the code keep a local file that would otherwise be
/// deleted, which is the safe direction.
fn path_hash(s: &str) -> u64 {
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// Disk-backed set used by mirror deletion. Only a small insertion buffer and
/// SQLite page cache stay in memory, regardless of the number of remote paths.
struct MirrorManifest {
    path: PathBuf,
    conn: Option<Connection>,
    pending: Vec<i64>,
}

impl MirrorManifest {
    fn create<R: Runtime>(app: &AppHandle<R>, job_id: &str) -> Result<Self, String> {
        let dir = app
            .path()
            .app_data_dir()
            .map_err(|e| format!("获取应用数据目录失败: {e}"))?
            .join("sync-manifests");
        fs::create_dir_all(&dir).map_err(|e| format!("创建镜像清单目录失败: {e}"))?;
        Self::create_at(dir.join(format!("mirror-{job_id}.sqlite")))
    }

    fn create_at(path: PathBuf) -> Result<Self, String> {
        let _ = fs::remove_file(&path);
        let conn = Connection::open(&path).map_err(|e| format!("创建镜像清单失败: {e}"))?;
        conn.execute_batch(
            "PRAGMA journal_mode=OFF;
             PRAGMA synchronous=OFF;
             PRAGMA locking_mode=EXCLUSIVE;
             PRAGMA cache_size=-16384;
             PRAGMA temp_store=FILE;
             CREATE TABLE seen (
               hash INTEGER PRIMARY KEY
             ) WITHOUT ROWID;",
        )
        .map_err(|e| format!("初始化镜像清单失败: {e}"))?;
        Ok(Self {
            path,
            conn: Some(conn),
            pending: Vec::with_capacity(MANIFEST_BATCH_SIZE),
        })
    }

    fn insert_path(&mut self, path: &str) -> Result<(), String> {
        self.pending.push(path_hash(path) as i64);
        if self.pending.len() >= MANIFEST_BATCH_SIZE {
            self.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), String> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let mut hashes = std::mem::take(&mut self.pending);
        hashes.sort_unstable();
        hashes.dedup();
        let conn = self
            .conn
            .as_mut()
            .ok_or_else(|| "镜像清单连接已关闭".to_string())?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("开始写入镜像清单失败: {e}"))?;
        {
            let mut stmt = tx
                .prepare_cached("INSERT OR IGNORE INTO seen(hash) VALUES (?1)")
                .map_err(|e| format!("准备镜像清单写入失败: {e}"))?;
            for hash in hashes {
                stmt.execute(params![hash])
                    .map_err(|e| format!("写入镜像清单失败: {e}"))?;
            }
        }
        tx.commit().map_err(|e| format!("提交镜像清单失败: {e}"))
    }

    fn contains_hash(&self, hash: u64) -> bool {
        let Some(conn) = self.conn.as_ref() else {
            return true;
        };
        // Query failures keep the local path, which is the safe direction for
        // mirror deletion.
        match conn
            .query_row(
                "SELECT 1 FROM seen WHERE hash = ?1",
                params![hash as i64],
                |_| Ok(1u8),
            )
            .optional()
        {
            Ok(Some(_)) => true,
            Ok(None) => false,
            Err(_) => true,
        }
    }
}

impl Drop for MirrorManifest {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            let _ = conn.close();
        }
        let _ = fs::remove_file(&self.path);
    }
}

/// Exact, disk-backed set of file paths already processed by the listing
/// callback. Unlike the mirror manifest, this must store the full path: a hash
/// collision here could otherwise suppress a real transfer.
struct ScanManifest {
    path: PathBuf,
    conn: Option<Connection>,
    pending: Vec<String>,
}

impl ScanManifest {
    fn create<R: Runtime>(app: &AppHandle<R>, job_id: &str) -> Result<Self, String> {
        let dir = app
            .path()
            .app_data_dir()
            .map_err(|e| format!("获取应用数据目录失败: {e}"))?
            .join("sync-manifests");
        fs::create_dir_all(&dir).map_err(|e| format!("创建扫描去重目录失败: {e}"))?;
        Self::create_at(dir.join(format!("scan-{job_id}.sqlite")))
    }

    fn create_at(path: PathBuf) -> Result<Self, String> {
        let _ = fs::remove_file(&path);
        let conn = Connection::open(&path).map_err(|e| format!("创建扫描去重数据库失败: {e}"))?;
        conn.execute_batch(
            "PRAGMA journal_mode=OFF;
             PRAGMA synchronous=OFF;
             PRAGMA locking_mode=EXCLUSIVE;
             PRAGMA cache_size=-32768;
             PRAGMA temp_store=FILE;
             CREATE TABLE scanned (
               path TEXT PRIMARY KEY
             ) WITHOUT ROWID;",
        )
        .map_err(|e| format!("初始化扫描去重数据库失败: {e}"))?;
        Ok(Self {
            path,
            conn: Some(conn),
            pending: Vec::with_capacity(MANIFEST_BATCH_SIZE),
        })
    }

    fn insert_path(&mut self, path: &str) -> Result<(), String> {
        self.pending.push(path.to_owned());
        if self.pending.len() >= MANIFEST_BATCH_SIZE {
            self.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), String> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let paths = std::mem::take(&mut self.pending);
        let conn = self
            .conn
            .as_mut()
            .ok_or_else(|| "扫描去重数据库连接已关闭".to_string())?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("开始写入扫描去重数据库失败: {e}"))?;
        {
            let mut stmt = tx
                .prepare_cached("INSERT OR IGNORE INTO scanned(path) VALUES (?1)")
                .map_err(|e| format!("准备扫描去重写入失败: {e}"))?;
            for path in paths {
                stmt.execute(params![path])
                    .map_err(|e| format!("写入扫描去重数据库失败: {e}"))?;
            }
        }
        tx.commit()
            .map_err(|e| format!("提交扫描去重数据库失败: {e}"))
    }

    fn contains_path(&self, path: &str) -> Result<bool, String> {
        let conn = self
            .conn
            .as_ref()
            .ok_or_else(|| "扫描去重数据库连接已关闭".to_string())?;
        conn.query_row(
            "SELECT 1 FROM scanned WHERE path = ?1",
            params![path],
            |_| Ok(1u8),
        )
        .optional()
        .map(|found| found.is_some())
        .map_err(|e| format!("查询扫描去重数据库失败: {e}"))
    }
}

impl Drop for ScanManifest {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            let _ = conn.close();
        }
        let _ = fs::remove_file(&self.path);
    }
}

trait PathPresence {
    fn contains_path_hash(&self, hash: u64) -> bool;
}

impl PathPresence for MirrorManifest {
    fn contains_path_hash(&self, hash: u64) -> bool {
        self.contains_hash(hash)
    }
}

#[cfg(test)]
impl PathPresence for HashSet<u64> {
    fn contains_path_hash(&self, hash: u64) -> bool {
        self.contains(&hash)
    }
}

/// A bounded FIFO set used to suppress an unexpected duplicate emitted within
/// the current listing pass. Cross-pass retry history lives in ScanManifest.
struct BoundedPathSet {
    set: HashSet<String>,
    order: VecDeque<String>,
    cap: usize,
}

impl BoundedPathSet {
    fn with_capacity(cap: usize) -> Self {
        Self {
            set: HashSet::with_capacity(cap),
            order: VecDeque::with_capacity(cap),
            cap: cap.max(1),
        }
    }

    fn contains(&self, path: &str) -> bool {
        self.set.contains(path)
    }

    fn insert(&mut self, path: String) {
        if self.set.contains(&path) {
            return;
        }
        if self.order.len() >= self.cap {
            if let Some(old) = self.order.pop_front() {
                self.set.remove(&old);
            }
        }
        self.set.insert(path.clone());
        self.order.push_back(path);
    }
}

/// Sleeps for `d`, returning early when `stop` is set so retries can be aborted.
fn sleep_or_stop(stop: &AtomicBool, d: Duration) {
    let deadline = Instant::now() + d;
    while !stop.load(Ordering::Relaxed) {
        let remain = deadline.saturating_duration_since(Instant::now());
        if remain.is_zero() {
            break;
        }
        thread::sleep(remain.min(Duration::from_millis(100)));
    }
}

/// Runs a full sync job to completion (or until `stop` is set). Blocks until
/// everything is finished; the caller should run this on a dedicated thread.
pub fn run_job<R: Runtime>(
    app: &AppHandle<R>,
    job_id: &str,
    opts: SyncOptions,
    stop: Arc<AtomicBool>,
    progress: Arc<Mutex<JobProgress>>,
) -> JobSummary {
    let local_root = PathBuf::from(&opts.local_dir);
    if let Err(e) = fs::create_dir_all(&local_root) {
        let mut p = progress.lock().unwrap();
        p.error = Some(format!("创建本地目录失败: {e}"));
        return JobSummary {
            status: JobStatus::Error,
            error: p.error.clone(),
            message: None,
        };
    }
    {
        let mut p = progress.lock().unwrap();
        p.phase = "scanning".into();
        p.activity = "正在连接服务端并扫描远端目录".into();
    }
    log_info(
        app,
        job_id,
        format!("开始同步：{} → {}", opts.share, opts.local_dir),
    );

    // Mirror deletion uses a temporary disk-backed manifest. A failed manifest
    // disables deletion for this run rather than falling back to unbounded RAM.
    let (mirror_manifest, initial_manifest_error) = if opts.delete_removed {
        match MirrorManifest::create(app, job_id) {
            Ok(manifest) => (Some(manifest), None),
            Err(error) => {
                log_warn(
                    app,
                    job_id,
                    format!("无法创建磁盘镜像清单，本次将禁用镜像删除：{error}"),
                );
                (None, Some(error))
            }
        }
    } else {
        (None, None)
    };
    // Optional exact-path manifest for safe full rescans after a partial
    // listing failure. If it cannot be created, the initial scan may continue,
    // but partial retries remain disabled because they would duplicate work.
    let (scan_manifest, initial_scan_manifest_error) = if opts.rescan_on_interrupt {
        match ScanManifest::create(app, job_id) {
            Ok(manifest) => {
                log_info(
                    app,
                    job_id,
                    "已启用扫描中断自动重扫：已扫描路径将写入本机磁盘数据库",
                );
                (Some(manifest), None)
            }
            Err(error) => {
                log_warn(
                    app,
                    job_id,
                    format!("无法创建扫描去重数据库，部分扫描后的自动重扫已禁用：{error}"),
                );
                (None, Some(error))
            }
        }
    } else {
        (None, None)
    };
    // Non-zero means the server completed a partial scan after giving up on
    // inaccessible paths. This is also a safety gate for mirror deletion.
    let remote_skipped_paths = Arc::new(AtomicU64::new(0));

    let thread_count = opts.threads.max(1).min(512);
    let (task_tx, task_rx) = bounded::<SyncTask>(thread_count * 16);
    let active_transfers: ActiveTransfers = Arc::new(Mutex::new(HashMap::with_capacity(
        thread_count.saturating_mul(2),
    )));
    // Monotonic count of bytes actually written by all workers, including
    // partial large files and retry attempts. Used only for live throughput.
    let transferred_bytes = Arc::new(AtomicU64::new(0));
    let emitter_done = Arc::new(AtomicBool::new(false));

    // Emitter thread: sends only bounded active snapshots and aggregate stats.
    let emitter = {
        let app = app.clone();
        let job_id = job_id.to_string();
        let progress = Arc::clone(&progress);
        let active_transfers = Arc::clone(&active_transfers);
        let transferred_bytes = Arc::clone(&transferred_bytes);
        let stop = Arc::clone(&stop);
        let emitter_done = Arc::clone(&emitter_done);
        thread::spawn(move || {
            emitter_loop(
                &app,
                &job_id,
                progress,
                active_transfers,
                transferred_bytes,
                stop,
                emitter_done,
            )
        })
    };

    // Listing thread: streams the remote file tree into the task queue. If the
    // connection drops mid-scan (e.g. os error 10054), the scan is retried a
    // few times when enabled instead of failing the whole job; an exact
    // disk-backed manifest prevents duplicate stats and transfer tasks.
    let listing = {
        let task_tx = task_tx.clone();
        let stop = Arc::clone(&stop);
        let progress = Arc::clone(&progress);
        let app = app.clone();
        let job_id = job_id.to_string();
        let l_opts = opts.clone();
        let mut mirror_manifest = mirror_manifest;
        let mut manifest_error = initial_manifest_error;
        let mut scan_manifest = scan_manifest;
        let mut scan_manifest_error = initial_scan_manifest_error;
        let remote_skipped_paths = Arc::clone(&remote_skipped_paths);
        // Bounded to the in-pipeline window (queue + in-flight workers) so it
        // stays tiny regardless of how many files are scanned.
        let sent_paths = Arc::new(Mutex::new(BoundedPathSet::with_capacity(
            thread_count * 16 + thread_count,
        )));
        thread::spawn(move || {
            let mut last_err: Option<String> = None;
            let mut ok = false;
            for attempt in 0..=MAX_LIST_RETRIES {
                let mut attempt_files = 0u64;
                let mut attempt_new_files = 0u64;
                let mut callback_error: Option<String> = None;
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                {
                    let mut p = progress.lock().unwrap();
                    p.list_attempt = attempt + 1;
                    p.phase = if attempt == 0 { "scanning" } else { "retrying" }.into();
                    p.activity = if attempt == 0 {
                        "正在扫描远端目录".into()
                    } else {
                        format!("正在准备第 {attempt}/{MAX_LIST_RETRIES} 次目录扫描重试")
                    };
                }
                if attempt > 0 {
                    log_info(
                        &app,
                        &job_id,
                        format!(
                            "扫描远端目录连接中断，{} 秒后进行第 {}/{} 次重扫；已扫描路径将从磁盘数据库去重",
                            LIST_RETRY_INTERVAL.as_secs(),
                            attempt,
                            MAX_LIST_RETRIES
                        ),
                    );
                    sleep_or_stop(&stop, LIST_RETRY_INTERVAL);
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                }
                let list_result = list_remote_files(
                    &l_opts.remote_ip,
                    l_opts.remote_port,
                    &l_opts.share,
                    |entry| {
                        if manifest_error.is_none() {
                            let insert_error = mirror_manifest
                                .as_mut()
                                .and_then(|manifest| manifest.insert_path(&entry.path).err());
                            if let Some(error) = insert_error {
                                manifest_error = Some(error);
                                mirror_manifest = None;
                            }
                        }
                        if stop.load(Ordering::Relaxed) {
                            return false;
                        }
                        if entry.is_dir {
                            return true;
                        }
                        attempt_files += 1;
                        if attempt > 0 {
                            if let Some(manifest) = scan_manifest.as_ref() {
                                match manifest.contains_path(&entry.path) {
                                    Ok(true) => return true,
                                    Ok(false) => {}
                                    Err(error) => {
                                        callback_error = Some(error);
                                        return false;
                                    }
                                }
                            }
                        }
                        if sent_paths.lock().unwrap().contains(&entry.path) {
                            return true;
                        }
                        if let Some(manifest) = scan_manifest.as_mut() {
                            if let Err(error) = manifest.insert_path(&entry.path) {
                                callback_error = Some(error);
                                return false;
                            }
                        }
                        attempt_new_files += 1;
                        let unique_scanned = {
                            let mut p = progress.lock().unwrap();
                            p.scanned_files += 1;
                            if p.scanned_files % 2048 == 0 && p.active_files == 0 {
                                p.activity = format!("正在扫描：{}", entry.path);
                            }
                            p.scanned_files
                        };
                        if unique_scanned % 100_000 == 0 {
                            log_info(
                                &app,
                                &job_id,
                                format!(
                                    "远端目录已扫描 {} 个不重复文件，当前：{}",
                                    unique_scanned, entry.path
                                ),
                            );
                        }
                        if l_opts.incremental {
                            // Skip if the local copy is already up to date.
                            // One stat per file, done inline: parallelizing
                            // stat rarely helps (stat is cheap and often the
                            // disk, not the CPU, is the limit) and only adds
                            // channel/thread overhead.
                            let lp = Path::new(&l_opts.local_dir).join(&entry.path);
                            if let Ok(md) = fs::metadata(&lp) {
                                if md.len() == entry.size && mtime_secs(&md) >= entry.mtime {
                                    let mut p = progress.lock().unwrap();
                                    p.skipped_files += 1;
                                    sent_paths.lock().unwrap().insert(entry.path.clone());
                                    return true;
                                }
                            }
                        }
                        {
                            let mut p = progress.lock().unwrap();
                            p.total_files += 1;
                            p.total_bytes += entry.size;
                        }
                        if task_tx
                            .send(SyncTask {
                                path: entry.path.clone(),
                                size: entry.size,
                            })
                            .is_err()
                        {
                            if !stop.load(Ordering::Relaxed) {
                                callback_error = Some("同步传输队列已关闭".into());
                            }
                            return false;
                        }
                        sent_paths.lock().unwrap().insert(entry.path.clone());
                        true
                    },
                );
                let mut local_failure = callback_error.is_some();
                let mut res = match callback_error.take() {
                    Some(error) => Err(error),
                    None => list_result,
                };
                // A network error can occur after the last in-memory SQLite
                // batch. Commit it before reconnecting so every processed path
                // is visible to the next full rescan.
                if res.is_err() && !local_failure && attempt_new_files > 0 {
                    if let Some(manifest) = scan_manifest.as_mut() {
                        if let Err(error) = manifest.flush() {
                            local_failure = true;
                            scan_manifest_error = Some(error.clone());
                            res = Err(error);
                        }
                    }
                }
                if scan_manifest_error.is_some() {
                    scan_manifest = None;
                }
                match res {
                    Ok((remote_files, remote_bytes, skipped_paths)) => {
                        ok = true;
                        remote_skipped_paths.store(skipped_paths, Ordering::Relaxed);
                        if !stop.load(Ordering::Relaxed) {
                            let mut p = progress.lock().unwrap();
                            p.listing_complete = true;
                            p.phase = if p.active_files > 0 || p.done_files < p.total_files {
                                "transferring"
                            } else {
                                "finalizing"
                            }
                            .into();
                            p.activity = if skipped_paths > 0 {
                                format!(
                                    "远端目录扫描完成：{remote_files} 个文件，已跳过 {skipped_paths} 个异常路径"
                                )
                            } else {
                                format!(
                                    "远端目录扫描完成：{remote_files} 个文件，正在等待传输队列结束"
                                )
                            };
                            drop(p);
                            let summary = format!(
                                "远端目录扫描完成：{remote_files} 个文件，{remote_bytes} 字节，服务端跳过 {skipped_paths} 个异常路径"
                            );
                            if skipped_paths > 0 {
                                log_warn(&app, &job_id, summary);
                            } else {
                                log_info(&app, &job_id, summary);
                            }
                        }
                        break;
                    }
                    Err(e) => {
                        last_err = Some(e);
                        log_error(
                            &app,
                            &job_id,
                            format!(
                                "扫描远端目录失败：{}",
                                last_err.as_deref().unwrap_or_default()
                            ),
                        );
                        if local_failure {
                            let message = format!(
                                "远端目录扫描因本机处理失败而中断，无法安全重扫：{}",
                                last_err.as_deref().unwrap_or_default()
                            );
                            let mut p = progress.lock().unwrap();
                            p.phase = "error".into();
                            p.activity = "本机扫描去重或传输队列异常".into();
                            if p.error.is_none() {
                                p.error = Some(message.clone());
                            }
                            drop(p);
                            log_error(&app, &job_id, message);
                            break;
                        }
                        let can_rescan_partial = l_opts.rescan_on_interrupt
                            && scan_manifest.is_some()
                            && scan_manifest_error.is_none();
                        if attempt_files > 0 && !can_rescan_partial {
                            let retry_reason = if l_opts.rescan_on_interrupt {
                                "扫描去重数据库不可用"
                            } else {
                                "未启用“扫描中断后自动重新扫描”"
                            };
                            let message = format!(
                                "远端目录扫描在本轮读取 {attempt_files} 个文件后中断（{retry_reason}）。为避免重复统计和重复传输，已结束本次任务：{}",
                                last_err.as_deref().unwrap_or_default()
                            );
                            let mut p = progress.lock().unwrap();
                            p.phase = "error".into();
                            p.activity = "远端目录扫描中断".into();
                            if p.error.is_none() {
                                p.error = Some(message.clone());
                            }
                            drop(p);
                            log_error(&app, &job_id, message);
                            break;
                        }
                        if attempt_files > 0 && attempt < MAX_LIST_RETRIES {
                            log_warn(
                                &app,
                                &job_id,
                                format!(
                                    "本轮扫描中断：读取 {attempt_files} 个文件，其中新增 {attempt_new_files} 个；即将从根目录重扫并使用磁盘数据库去重"
                                ),
                            );
                        }
                    }
                }
            }
            if !ok && !stop.load(Ordering::Relaxed) {
                let mut p = progress.lock().unwrap();
                if p.error.is_none() {
                    p.error = Some(format!(
                        "扫描远端目录失败（已重试 {} 次）: {}",
                        MAX_LIST_RETRIES,
                        last_err.unwrap_or_default()
                    ));
                    p.phase = "error".into();
                    p.activity = "无法连接远端目录扫描服务".into();
                }
            }
            if manifest_error.is_none() {
                let flush_error = mirror_manifest
                    .as_mut()
                    .and_then(|manifest| manifest.flush().err());
                if let Some(error) = flush_error {
                    manifest_error = Some(error);
                    mirror_manifest = None;
                }
            }
            (mirror_manifest, manifest_error)
        })
    };
    drop(task_tx);

    // Main download workers: they never block on retries. Failed files are
    // pushed into a retry queue handled by a dedicated pool (20% of threads,
    // min 1), which scales up to the full pool once the normal transfer ends.
    let limiter = Arc::new(BandwidthLimiter::new(
        opts.bandwidth_mbps.saturating_mul(1024 * 1024),
    ));
    let retry_queue = Arc::new(RetryQueue::new());
    let main_done = Arc::new(AtomicBool::new(false));

    let mut workers = Vec::new();
    for _ in 0..thread_count {
        let rx = task_rx.clone();
        let stop = Arc::clone(&stop);
        let progress = Arc::clone(&progress);
        let active_transfers = Arc::clone(&active_transfers);
        let transferred_bytes = Arc::clone(&transferred_bytes);
        let limiter = Arc::clone(&limiter);
        let retry_queue = Arc::clone(&retry_queue);
        let w_opts = opts.clone();
        let app = app.clone();
        let job_id = job_id.to_string();
        workers.push(thread::spawn(move || loop {
            let task = match rx.recv_timeout(Duration::from_millis(500)) {
                Ok(t) => t,
                Err(RecvTimeoutError::Timeout) => {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => break,
            };
            transfer_started(&progress, &active_transfers, &task);
            let result = download_file(
                &w_opts,
                &limiter,
                &stop,
                &task,
                &active_transfers,
                &transferred_bytes,
            );
            transfer_finished(&progress, &active_transfers, &task.path);
            match result {
                Ok(()) => {
                    let mut p = progress.lock().unwrap();
                    p.done_files += 1;
                    p.done_bytes += task.size;
                }
                Err(e) => {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    retry_queue.push(RetryTask {
                        path: task.path.clone(),
                        size: task.size,
                        attempt: 1,
                        next_attempt_at: Instant::now() + RETRY_INTERVAL,
                    });
                    let _ = app.emit(
                        EVT_RETRY,
                        RetryPayload {
                            id: job_id.clone(),
                            path: task.path.clone(),
                            attempt: 1,
                            max_retries: MAX_RETRIES,
                            retry_in: RETRY_INTERVAL.as_secs(),
                            state: "retrying".into(),
                        },
                    );
                    log_warn(
                        &app,
                        &job_id,
                        format!(
                            "{} 下载失败：{}，{} 秒后进行第 1/{} 次重试",
                            task.path,
                            e,
                            RETRY_INTERVAL.as_secs(),
                            MAX_RETRIES
                        ),
                    );
                }
            }
        }));
    }

    // Dedicated retry pool: at most 20% of the thread count, at least 1.
    let retry_threads = ((thread_count * 20) / 100).max(1);
    let mut retry_workers = Vec::new();
    for _ in 0..retry_threads {
        retry_workers.push(spawn_retry_worker(
            app,
            job_id,
            &opts,
            &limiter,
            &stop,
            &progress,
            &active_transfers,
            &transferred_bytes,
            &retry_queue,
            &main_done,
        ));
    }

    let (mirror_manifest, manifest_error) = match listing.join() {
        Ok(result) => result,
        Err(_) => (
            None,
            Some("目录扫描线程异常退出，镜像清单不可用".to_string()),
        ),
    };
    for w in workers {
        let _ = w.join();
    }

    {
        let mut p = progress.lock().unwrap();
        if p.error.is_none() && !stop.load(Ordering::Relaxed) {
            if retry_queue.is_empty() {
                p.phase = "finalizing".into();
                p.activity = "传输队列已清空，正在汇总结果".into();
            } else {
                p.phase = "retrying".into();
                p.activity = "正在处理失败文件重试队列".into();
            }
        }
    }
    log_info(app, job_id, "远端扫描与主传输队列已结束，正在处理收尾任务");

    // Normal transfers finished: drain the retry queue with the full pool.
    main_done.store(true, Ordering::Relaxed);
    retry_queue.notify_all();
    for _ in 0..thread_count.saturating_sub(retry_threads) {
        retry_workers.push(spawn_retry_worker(
            app,
            job_id,
            &opts,
            &limiter,
            &stop,
            &progress,
            &active_transfers,
            &transferred_bytes,
            &retry_queue,
            &main_done,
        ));
    }
    retry_queue.notify_all();
    for w in retry_workers {
        let _ = w.join();
    }

    {
        let mut p = progress.lock().unwrap();
        if p.error.is_none() && !stop.load(Ordering::Relaxed) {
            p.phase = "finalizing".into();
            p.activity = "正在写入最终统计并检查镜像删除".into();
        }
    }
    emitter_done.store(true, Ordering::Relaxed);
    let _ = emitter.join();

    // Mirror-deletion: remove local files/dirs that no longer exist on the remote.
    if opts.delete_removed && !stop.load(Ordering::Relaxed) {
        let skipped_paths = remote_skipped_paths.load(Ordering::Relaxed);
        let has_error = progress.lock().unwrap().error.is_some();
        if skipped_paths > 0 {
            log_warn(
                app,
                job_id,
                format!(
                    "服务端跳过了 {skipped_paths} 个无法访问路径，本次已禁用镜像删除以防误删本地文件"
                ),
            );
        } else if let Some(error) = manifest_error.as_deref() {
            log_warn(
                app,
                job_id,
                format!("磁盘镜像清单不可用，本次已禁用镜像删除以防误删：{error}"),
            );
        } else if !has_error {
            if let Some(manifest) = mirror_manifest.as_ref() {
                let payload = {
                    let mut p = progress.lock().unwrap();
                    p.phase = "deleting".into();
                    p.activity = "正在删除远端已不存在的本地文件".into();
                    job_payload(job_id, JobStatus::Running, &p, None)
                };
                let _ = app.emit(EVT_JOB, payload);
                log_info(app, job_id, "开始按磁盘镜像清单检查本地残留文件");
                let (del_files, del_dirs) = delete_missing(&local_root, manifest);
                if del_files > 0 || del_dirs > 0 {
                    log_info(
                        app,
                        job_id,
                        format!("镜像删除完成：删除 {del_files} 个文件、{del_dirs} 个目录"),
                    );
                }
            }
        }
    }

    let remote_skipped_path_count = remote_skipped_paths.load(Ordering::Relaxed);
    let mut p = progress.lock().unwrap();
    let status = if stop.load(Ordering::Relaxed) {
        JobStatus::Stopped
    } else if p.error.is_some() {
        JobStatus::Error
    } else {
        JobStatus::Finished
    };
    match status {
        JobStatus::Finished => {
            p.phase = "finished".into();
            p.activity = "同步已完成".into();
        }
        JobStatus::Stopped => {
            p.phase = "stopped".into();
            p.activity = "同步已停止".into();
        }
        JobStatus::Error => {
            p.phase = "error".into();
            p.activity = "同步失败，请查看错误日志".into();
        }
        JobStatus::Running => {}
    }
    p.current_file = None;
    p.active_files = 0;
    let partial_success =
        status == JobStatus::Finished && (p.failed_files > 0 || remote_skipped_path_count > 0);
    let message = if partial_success {
        Some(match (p.failed_files, remote_skipped_path_count) {
            (failed, skipped) if failed > 0 && skipped > 0 => format!(
                "同步已尽可能完成：{failed} 个文件重试 {MAX_RETRIES} 次后失败，服务端跳过 {skipped} 个异常路径"
            ),
            (failed, _) if failed > 0 => format!(
                "同步已尽可能完成：{failed} 个文件重试 {MAX_RETRIES} 次后失败并已跳过"
            ),
            (_, skipped) => format!(
                "同步已尽可能完成：服务端在连续失败 {MAX_PATH_FAILURES} 次后跳过 {skipped} 个异常路径"
            ),
        })
    } else if status == JobStatus::Finished && p.total_files == 0 && p.skipped_files > 0 {
        Some(format!(
            "所有文件已是最新，无需传输（跳过 {} 个文件）",
            p.skipped_files
        ))
    } else {
        None
    };
    log_info(
        app,
        job_id,
        format!("同步线程已退出（{}）", status.as_str()),
    );
    if let Some(m) = &message {
        if partial_success {
            log_warn(app, job_id, m.clone());
        } else {
            log_info(app, job_id, m.clone());
        }
    }
    JobSummary {
        status,
        error: p.error.clone(),
        message,
    }
}

/// One worker of the retry pool: takes a failed file once its backoff has
/// elapsed, retries the download, and re-queues or fails it by attempt count.
#[allow(clippy::too_many_arguments)]
fn spawn_retry_worker<R: Runtime>(
    app: &AppHandle<R>,
    job_id: &str,
    opts: &SyncOptions,
    limiter: &Arc<BandwidthLimiter>,
    stop: &Arc<AtomicBool>,
    progress: &Arc<Mutex<JobProgress>>,
    active_transfers: &ActiveTransfers,
    transferred_bytes: &Arc<AtomicU64>,
    queue: &Arc<RetryQueue>,
    main_done: &Arc<AtomicBool>,
) -> thread::JoinHandle<()> {
    let app = app.clone();
    let job_id = job_id.to_string();
    let opts = opts.clone();
    let limiter = Arc::clone(limiter);
    let stop = Arc::clone(stop);
    let progress = Arc::clone(progress);
    let active_transfers = Arc::clone(active_transfers);
    let transferred_bytes = Arc::clone(transferred_bytes);
    let queue = Arc::clone(queue);
    let main_done = Arc::clone(main_done);
    thread::spawn(move || {
        while let Some(task) = queue.pop_ready(&stop, &main_done) {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            let attempt = task.attempt;
            let _ = app.emit(
                EVT_RETRY,
                RetryPayload {
                    id: job_id.clone(),
                    path: task.path.clone(),
                    attempt,
                    max_retries: MAX_RETRIES,
                    retry_in: 0,
                    state: "retrying".into(),
                },
            );
            let sync_task = SyncTask {
                path: task.path.clone(),
                size: task.size,
            };
            transfer_started(&progress, &active_transfers, &sync_task);
            let result = download_file(
                &opts,
                &limiter,
                &stop,
                &sync_task,
                &active_transfers,
                &transferred_bytes,
            );
            transfer_finished(&progress, &active_transfers, &sync_task.path);
            match result {
                Ok(()) => {
                    {
                        let mut p = progress.lock().unwrap();
                        p.done_files += 1;
                        p.done_bytes += task.size;
                    }
                    let _ = app.emit(
                        EVT_RETRY,
                        RetryPayload {
                            id: job_id.clone(),
                            path: task.path.clone(),
                            attempt,
                            max_retries: MAX_RETRIES,
                            retry_in: 0,
                            state: "succeeded".into(),
                        },
                    );
                }
                Err(e) => {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    if attempt < MAX_RETRIES {
                        queue.push(RetryTask {
                            path: task.path.clone(),
                            size: task.size,
                            attempt: attempt + 1,
                            next_attempt_at: Instant::now() + RETRY_INTERVAL,
                        });
                        let _ = app.emit(
                            EVT_RETRY,
                            RetryPayload {
                                id: job_id.clone(),
                                path: task.path.clone(),
                                attempt: attempt + 1,
                                max_retries: MAX_RETRIES,
                                retry_in: RETRY_INTERVAL.as_secs(),
                                state: "retrying".into(),
                            },
                        );
                        log_warn(
                            &app,
                            &job_id,
                            format!(
                                "{} 下载失败：{}，{} 秒后进行第 {}/{} 次重试",
                                task.path,
                                e,
                                RETRY_INTERVAL.as_secs(),
                                attempt + 1,
                                MAX_RETRIES
                            ),
                        );
                    } else {
                        let mut p = progress.lock().unwrap();
                        p.failed_files += 1;
                        drop(p);
                        let _ = app.emit(
                            EVT_RETRY,
                            RetryPayload {
                                id: job_id.clone(),
                                path: task.path.clone(),
                                attempt,
                                max_retries: MAX_RETRIES,
                                retry_in: 0,
                                state: "failed".into(),
                            },
                        );
                        log_error(
                            &app,
                            &job_id,
                            format!(
                                "{} 下载失败：{}（已重试 {} 次，放弃）",
                                task.path, e, MAX_RETRIES
                            ),
                        );
                    }
                }
            }
        }
    })
}

/// Recursively delete local entries whose relative path is not present on the
/// remote share. Only aggregate counts are retained, so deletion is bounded.
fn delete_missing<P: PathPresence>(root: &Path, seen: &P) -> (u64, u64) {
    let mut files = 0u64;
    let mut dirs = 0u64;
    visit_delete(root, root, seen, &mut files, &mut dirs);
    (files, dirs)
}

fn rel_of(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .map(|x| x.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default()
}

fn visit_delete<P: PathPresence>(
    root: &Path,
    dir: &Path,
    seen: &P,
    files: &mut u64,
    dirs: &mut u64,
) {
    let rel = rel_of(root, dir);
    if !rel.is_empty() && !seen.contains_path_hash(path_hash(&rel)) {
        // The whole subtree no longer exists on the remote side.
        if fs::remove_dir_all(dir).is_ok() {
            *dirs += 1;
        }
        return;
    }
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for e in entries.flatten() {
        let md = match e.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let rrel = rel_of(root, &e.path());
        if md.is_dir() {
            visit_delete(root, &e.path(), seen, files, dirs);
        } else if !seen.contains_path_hash(path_hash(&rrel)) && fs::remove_file(e.path()).is_ok() {
            *files += 1;
        }
    }
}

/// Download a single file over its own TCP connection. Progress is written
/// into a bounded active map; only the emitter thread crosses the WebView IPC.
fn download_file(
    opts: &SyncOptions,
    limiter: &BandwidthLimiter,
    stop: &AtomicBool,
    task: &SyncTask,
    active_transfers: &ActiveTransfers,
    transferred_bytes: &AtomicU64,
) -> Result<(), String> {
    let mut stream = connect_with_timeout(&opts.remote_ip, opts.remote_port, 15)?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(60)))
        .map_err(|e| e.to_string())?;

    write_msg(
        &mut stream,
        &ClientMsg::Hello {
            version: PROTOCOL_VERSION,
        },
    )
    .map_err(|e| e.to_string())?;
    match read_msg::<_, ServerMsg>(&mut stream).map_err(|e| e.to_string())? {
        Some(ServerMsg::HelloAck { .. }) => {}
        Some(ServerMsg::Error { message }) => return Err(message),
        _ => return Err("服务器响应异常".into()),
    }
    write_msg(
        &mut stream,
        &ClientMsg::FetchFile {
            share: opts.share.clone(),
            path: task.path.clone(),
        },
    )
    .map_err(|e| e.to_string())?;
    let size = match read_msg::<_, ServerMsg>(&mut stream).map_err(|e| e.to_string())? {
        Some(ServerMsg::FileMeta { size, .. }) => size,
        Some(ServerMsg::Error { message }) => return Err(message),
        _ => return Err("服务器响应异常".into()),
    };

    let dest = Path::new(&opts.local_dir).join(&task.path);
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {e}"))?;
    }
    let mut file =
        File::create(&dest).map_err(|e| format!("创建文件失败 {}: {e}", dest.display()))?;

    // Use a short read timeout so a stop request is honored promptly even if
    // the server stalls; timeouts just retry the read instead of aborting.
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));

    let mut remaining = size;
    let mut buf = vec![0u8; CHUNK];
    let file_start = Instant::now();
    let mut last_emit = Instant::now();
    let mut last_pct: u64 = 0;
    let mut file_done: u64 = 0;

    while remaining > 0 {
        if stop.load(Ordering::Relaxed) {
            return Err("已停止".into());
        }
        let to_read = remaining.min(buf.len() as u64) as usize;
        let n = match stream.read(&mut buf[..to_read]) {
            Ok(n) => n,
            Err(e)
                if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut =>
            {
                continue; // no data yet; re-check stop at the loop top
            }
            Err(e) => return Err(format!("读取网络数据失败: {e}")),
        };
        if n == 0 {
            return Err("文件传输中断（服务器提前关闭连接）".into());
        }
        limiter.acquire(n as u64);
        // Write in sub-chunks so a slow disk cannot delay a stop request for long.
        let mut written = 0usize;
        while written < n {
            if stop.load(Ordering::Relaxed) {
                return Err("已停止".into());
            }
            let end = (written + 64 * 1024).min(n);
            file.write_all(&buf[written..end])
                .map_err(|e| format!("写入本地文件失败: {e}"))?;
            written = end;
        }
        remaining -= n as u64;
        file_done += n as u64;
        transferred_bytes.fetch_add(n as u64, Ordering::Relaxed);

        // Updating this in-memory entry is cheap and bounded by worker count.
        // The emitter snapshots the entire map at a fixed low frequency.
        let pct = file_done.saturating_mul(100) / size;
        let elapsed = last_emit.elapsed();
        if (pct > last_pct && elapsed >= Duration::from_millis(200))
            || elapsed >= Duration::from_secs(2)
        {
            let speed = (file_done as f64 / file_start.elapsed().as_secs_f64().max(0.001)) as u64;
            if let Some(item) = active_transfers.lock().unwrap().get_mut(&task.path) {
                item.done = file_done;
                item.total = size;
                item.speed = speed;
            }
            last_pct = pct;
            last_emit = Instant::now();
        }
    }

    Ok(())
}

fn emitter_loop<R: Runtime>(
    app: &AppHandle<R>,
    job_id: &str,
    progress: Arc<Mutex<JobProgress>>,
    active_transfers: ActiveTransfers,
    transferred_bytes: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
) {
    let mut last_emit = Instant::now();
    let mut speed_time = Instant::now();
    let mut last_transferred_bytes: u64 = 0;

    loop {
        let stopped = stop.load(Ordering::Relaxed);
        let finished = done.load(Ordering::Relaxed);
        if stopped || finished || last_emit.elapsed() >= PROGRESS_EMIT_INTERVAL {
            let mut files: Vec<_> = if stopped {
                Vec::new()
            } else {
                active_transfers.lock().unwrap().values().cloned().collect()
            };
            files.sort_unstable_by(|a, b| a.path.cmp(&b.path));
            let _ = app.emit(
                EVT_PROGRESS,
                FileProgressPayload {
                    id: job_id.to_string(),
                    files,
                },
            );

            let mut p = progress.lock().unwrap();
            let dt = speed_time.elapsed().as_secs_f64().max(0.001);
            let current_transferred_bytes = transferred_bytes.load(Ordering::Relaxed);
            p.speed = if stopped || finished {
                0
            } else {
                (current_transferred_bytes.saturating_sub(last_transferred_bytes) as f64 / dt)
                    as u64
            };
            speed_time = Instant::now();
            last_transferred_bytes = current_transferred_bytes;
            let status = if stopped {
                JobStatus::Stopped
            } else {
                JobStatus::Running
            };
            let payload = job_payload(job_id, status, &p, None);
            drop(p);
            let _ = app.emit(EVT_JOB, payload);
            last_emit = Instant::now();

            if stopped || finished {
                break;
            }
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn transfer_started(
    progress: &Arc<Mutex<JobProgress>>,
    active_transfers: &ActiveTransfers,
    task: &SyncTask,
) {
    {
        let mut p = progress.lock().unwrap();
        p.active_files += 1;
        p.current_file = Some(task.path.clone());
        if p.listing_complete {
            p.phase = "transferring".into();
        }
        p.activity = format!("正在传输：{}", task.path);
    }
    active_transfers.lock().unwrap().insert(
        task.path.clone(),
        ActiveFileProgress {
            path: task.path.clone(),
            done: 0,
            total: task.size,
            speed: 0,
        },
    );
}

fn transfer_finished(
    progress: &Arc<Mutex<JobProgress>>,
    active_transfers: &ActiveTransfers,
    path: &str,
) {
    active_transfers.lock().unwrap().remove(path);
    let mut p = progress.lock().unwrap();
    p.active_files = p.active_files.saturating_sub(1);
    if p.active_files == 0 {
        p.current_file = None;
        if p.listing_complete {
            if p.done_files < p.total_files {
                p.activity = "正在等待传输队列".into();
            } else {
                p.phase = "finalizing".into();
                p.activity = "传输已完成，正在等待远端目录扫描结束".into();
            }
        } else {
            p.activity = "正在继续扫描远端目录".into();
        }
    }
}

fn job_payload(
    job_id: &str,
    status: JobStatus,
    p: &JobProgress,
    message: Option<String>,
) -> JobEventPayload {
    JobEventPayload {
        id: job_id.to_string(),
        status,
        total_files: p.total_files,
        done_files: p.done_files,
        failed_files: p.failed_files,
        skipped_files: p.skipped_files,
        total_bytes: p.total_bytes,
        done_bytes: p.done_bytes,
        speed: p.speed,
        scanned_files: p.scanned_files,
        active_files: p.active_files,
        listing_complete: p.listing_complete,
        list_attempt: p.list_attempt,
        phase: p.phase.clone(),
        activity: p.activity.clone(),
        current_file: p.current_file.clone(),
        message,
    }
}

pub fn log_info<R: Runtime>(app: &AppHandle<R>, job_id: &str, message: impl Into<String>) {
    logging::emit(app, job_id, "client", "info", message);
}

pub fn log_warn<R: Runtime>(app: &AppHandle<R>, job_id: &str, message: impl Into<String>) {
    logging::emit(app, job_id, "client", "warn", message);
}

pub fn log_error<R: Runtime>(app: &AppHandle<R>, job_id: &str, message: impl Into<String>) {
    logging::emit(app, job_id, "client", "error", message);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirror_manifest_is_disk_backed_and_cleaned_up() {
        let path = std::env::temp_dir().join(format!(
            "bbdduck-mirror-manifest-test-{}.sqlite",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);
        {
            let mut manifest = MirrorManifest::create_at(path.clone()).unwrap();
            manifest.insert_path("keep/a.txt").unwrap();
            manifest.insert_path("keep/a.txt").unwrap();
            manifest.insert_path("keep/sub/b.txt").unwrap();
            manifest.flush().unwrap();
            assert!(manifest.contains_hash(path_hash("keep/a.txt")));
            assert!(manifest.contains_hash(path_hash("keep/sub/b.txt")));
            assert!(!manifest.contains_hash(path_hash("missing.txt")));
        }
        assert!(
            !path.exists(),
            "temporary manifest should be removed on drop"
        );
    }

    #[test]
    fn scan_manifest_tracks_exact_paths_and_is_cleaned_up() {
        let path = std::env::temp_dir().join(format!(
            "bbdduck-scan-manifest-test-{}.sqlite",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);
        {
            let mut manifest = ScanManifest::create_at(path.clone()).unwrap();
            manifest.insert_path("folder/report.txt").unwrap();
            manifest.insert_path("folder/report (copy).txt").unwrap();
            manifest.flush().unwrap();

            assert!(manifest.contains_path("folder/report.txt").unwrap());
            assert!(manifest.contains_path("folder/report (copy).txt").unwrap());
            assert!(!manifest.contains_path("folder/REPORT.txt").unwrap());
            assert!(!manifest.contains_path("folder/missing.txt").unwrap());
        }
        assert!(
            !path.exists(),
            "temporary scan manifest should be removed on drop"
        );
    }

    #[test]
    fn delete_missing_removes_leftovers_keeps_mirrored() {
        let root = std::env::temp_dir().join(format!("bbdduck-del-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("keep/sub")).unwrap();
        fs::write(root.join("keep/a.txt"), b"a").unwrap();
        fs::write(root.join("keep/sub/b.txt"), b"b").unwrap();
        fs::write(root.join("stale.txt"), b"s").unwrap();
        fs::create_dir_all(root.join("stale_dir")).unwrap();
        fs::write(root.join("stale_dir/c.txt"), b"c").unwrap();

        let seen: HashSet<u64> = ["keep", "keep/a.txt", "keep/sub", "keep/sub/b.txt"]
            .iter()
            .map(|s| path_hash(s))
            .collect();
        let (files, dirs) = delete_missing(&root, &seen);

        assert_eq!(files, 1, "only stale.txt should be removed");
        assert_eq!(dirs, 1, "only stale_dir should be removed");
        assert!(root.join("keep/a.txt").exists());
        assert!(root.join("keep/sub/b.txt").exists());
        assert!(!root.join("stale.txt").exists());
        assert!(!root.join("stale_dir").exists());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn retry_queue_pops_ready_task_skipping_waiting_head() {
        let q = RetryQueue::new();
        let stop = AtomicBool::new(false);
        let main_done = AtomicBool::new(false);

        // head is still in backoff; a later task is already ready
        q.push(RetryTask {
            path: "slow".into(),
            size: 1,
            attempt: 1,
            next_attempt_at: Instant::now() + Duration::from_secs(60),
        });
        q.push(RetryTask {
            path: "ready".into(),
            size: 2,
            attempt: 2,
            next_attempt_at: Instant::now(),
        });

        // pop_ready must return the ready task even though it is behind the head
        let popped = q.pop_ready(&stop, &main_done);
        let t = popped.expect("ready task should pop");
        assert_eq!(t.path, "ready");
        assert_eq!(t.attempt, 2);

        // with stop set, pop_ready returns None immediately
        stop.store(true, Ordering::Relaxed);
        assert!(q.pop_ready(&stop, &main_done).is_none());
    }
}
