//! The sync engine (Node B side): scans the remote share, compares with the
//! local folder (incremental), downloads files with N parallel workers under a
//! shared bandwidth cap, and emits progress events to the frontend.

use std::collections::{HashSet, VecDeque};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, Receiver, RecvTimeoutError, Sender};
use tauri::{AppHandle, Emitter, Runtime};

use super::client::{list_remote_files, BandwidthLimiter};
use super::model::*;
use super::protocol::{
    connect_with_timeout, mtime_secs, read_msg, write_msg, ClientMsg, ServerMsg, PROTOCOL_VERSION,
};

const CHUNK: usize = 256 * 1024;
/// Max retries per file after the first failure.
const MAX_RETRIES: u32 = 3;
/// Delay between retry attempts.
const RETRY_INTERVAL: Duration = Duration::from_secs(3);

#[derive(Default)]
pub struct JobProgress {
    pub total_files: u64,
    pub done_files: u64,
    pub failed_files: u64,
    pub skipped_files: u64,
    pub total_bytes: u64,
    pub done_bytes: u64,
    pub speed: u64,
    pub error: Option<String>,
}

pub struct JobSummary {
    pub status: JobStatus,
    pub error: Option<String>,
    pub message: Option<String>,
}

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
    log_info(app, job_id, format!("开始同步：{} → {}", opts.share, opts.local_dir));

    // Relative paths present on the remote, collected during the scan. Only
    // used when mirror-deletion is enabled, to remove local leftovers.
    let seen: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));

    let thread_count = opts.threads.max(1).min(64);
    let (task_tx, task_rx) = bounded::<SyncTask>(thread_count * 16);
    let (done_tx, done_rx) = bounded::<FileDone>(8192);

    // Emitter thread: batches completed-file events and emits periodic job stats.
    let emitter = {
        let app = app.clone();
        let job_id = job_id.to_string();
        let progress = Arc::clone(&progress);
        let stop = Arc::clone(&stop);
        thread::spawn(move || emitter_loop(&app, &job_id, done_rx, progress, stop))
    };

    // Listing thread: streams the remote file tree into the task queue.
    let listing = {
        let task_tx = task_tx.clone();
        let stop = Arc::clone(&stop);
        let progress = Arc::clone(&progress);
        let l_opts = opts.clone();
        let seen = Arc::clone(&seen);
        thread::spawn(move || {
            let res = list_remote_files(
                &l_opts.remote_ip,
                l_opts.remote_port,
                &l_opts.share,
                |entry| {
                    if l_opts.delete_removed {
                        seen.lock().unwrap().insert(entry.path.clone());
                    }
                    if stop.load(Ordering::Relaxed) {
                        return false;
                    }
                    if entry.is_dir {
                        return true;
                    }
                    if l_opts.incremental {
                        let lp = Path::new(&l_opts.local_dir).join(&entry.path);
                        if let Ok(md) = fs::metadata(&lp) {
                            if md.len() == entry.size && mtime_secs(&md) >= entry.mtime {
                                let mut p = progress.lock().unwrap();
                                p.skipped_files += 1;
                                return true;
                            }
                        }
                    }
                    {
                        let mut p = progress.lock().unwrap();
                        p.total_files += 1;
                        p.total_bytes += entry.size;
                    }
                    task_tx
                        .send(SyncTask {
                            path: entry.path.clone(),
                            size: entry.size,
                        })
                        .is_ok()
                },
            );
            match res {
                Ok(_) => {}
                Err(e) => {
                    let mut p = progress.lock().unwrap();
                    if p.error.is_none() {
                        p.error = Some(e);
                    }
                }
            }
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
        let limiter = Arc::clone(&limiter);
        let done_tx = done_tx.clone();
        let retry_queue = Arc::clone(&retry_queue);
        let w_opts = opts.clone();
        let app = app.clone();
        let job_id = job_id.to_string();
        workers.push(thread::spawn(move || {
            loop {
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
                match download_file(&app, &job_id, &w_opts, &limiter, &stop, &task, &done_tx) {
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
                        log_info(
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
            &done_tx,
            &retry_queue,
            &main_done,
        ));
    }

    let _ = listing.join();
    for w in workers {
        let _ = w.join();
    }

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
            &done_tx,
            &retry_queue,
            &main_done,
        ));
    }
    retry_queue.notify_all();
    for w in retry_workers {
        let _ = w.join();
    }

    drop(done_tx);
    let _ = emitter.join();

    // Mirror-deletion: remove local files/dirs that no longer exist on the remote.
    if opts.delete_removed && !stop.load(Ordering::Relaxed) {
        let has_error = progress.lock().unwrap().error.is_some();
        if !has_error {
            let mut deleted_paths: Vec<String> = Vec::new();
            let (del_files, del_dirs) = {
                let seen = seen.lock().unwrap();
                delete_missing(&local_root, &seen, &mut deleted_paths)
            };
            // Stream deletion events to the frontend in chunks.
            for chunk in deleted_paths.chunks(2000) {
                let _ = app.emit(
                    EVT_FILES_DELETED,
                    FilesDeletedPayload {
                        id: job_id.to_string(),
                        files: chunk.to_vec(),
                    },
                );
            }
            if del_files > 0 || del_dirs > 0 {
                log_info(
                    app,
                    job_id,
                    format!("镜像删除完成：删除 {del_files} 个文件、{del_dirs} 个目录"),
                );
            }
        }
    }

    let p = progress.lock().unwrap();
    let status = if stop.load(Ordering::Relaxed) {
        JobStatus::Stopped
    } else if p.error.is_some() {
        JobStatus::Error
    } else {
        JobStatus::Finished
    };
    let message = if status == JobStatus::Finished && p.total_files == 0 && p.skipped_files > 0 {
        Some(format!(
            "所有文件已是最新，无需传输（跳过 {} 个文件）",
            p.skipped_files
        ))
    } else {
        None
    };
    log_info(app, job_id, format!("同步线程已退出（{}）", status.as_str()));
    if let Some(m) = &message {
        log_info(app, job_id, m.clone());
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
    done_tx: &Sender<FileDone>,
    queue: &Arc<RetryQueue>,
    main_done: &Arc<AtomicBool>,
) -> thread::JoinHandle<()> {
    let app = app.clone();
    let job_id = job_id.to_string();
    let opts = opts.clone();
    let limiter = Arc::clone(limiter);
    let stop = Arc::clone(stop);
    let progress = Arc::clone(progress);
    let done_tx = done_tx.clone();
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
            match download_file(&app, &job_id, &opts, &limiter, &stop, &sync_task, &done_tx) {
                Ok(()) => {
                    let mut p = progress.lock().unwrap();
                    p.done_files += 1;
                    p.done_bytes += task.size;
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
                        log_info(
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
                        if p.error.is_none() {
                            p.error = Some(format!("{}: {e}", task.path));
                        }
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
/// remote share (used for mirror sync). Deleted paths are pushed to `deleted`.
/// Returns (deleted_files, deleted_dirs).
fn delete_missing(root: &Path, seen: &HashSet<String>, deleted: &mut Vec<String>) -> (u64, u64) {
    let mut files = 0u64;
    let mut dirs = 0u64;
    visit_delete(root, root, seen, &mut files, &mut dirs, deleted);
    (files, dirs)
}

fn rel_of(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .map(|x| x.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default()
}

fn visit_delete(
    root: &Path,
    dir: &Path,
    seen: &HashSet<String>,
    files: &mut u64,
    dirs: &mut u64,
    deleted: &mut Vec<String>,
) {
    let rel = rel_of(root, dir);
    if !rel.is_empty() && !seen.contains(&rel) {
        // The whole subtree no longer exists on the remote side.
        if fs::remove_dir_all(dir).is_ok() {
            *dirs += 1;
            deleted.push(rel);
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
            visit_delete(root, &e.path(), seen, files, dirs, deleted);
        } else if !seen.contains(&rrel) {
            if fs::remove_file(&e.path()).is_ok() {
                *files += 1;
                deleted.push(rrel);
            }
        }
    }
}

/// Download a single file over its own TCP connection.
#[allow(clippy::too_many_arguments)]
fn download_file<R: Runtime>(
    app: &AppHandle<R>,
    job_id: &str,
    opts: &SyncOptions,
    limiter: &BandwidthLimiter,
    stop: &AtomicBool,
    task: &SyncTask,
    done_tx: &Sender<FileDone>,
) -> Result<(), String> {
    let mut stream = connect_with_timeout(&opts.remote_ip, opts.remote_port, 5)?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(60)))
        .map_err(|e| e.to_string())?;

    write_msg(&mut stream, &ClientMsg::Hello { version: PROTOCOL_VERSION })
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
    let mut file = File::create(&dest).map_err(|e| format!("创建文件失败 {}: {e}", dest.display()))?;

    // Use a short read timeout so a stop request is honored promptly even if
    // the server stalls; timeouts just retry the read instead of aborting.
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));

    let mut remaining = size;
    let mut buf = vec![0u8; CHUNK];
    let file_start = Instant::now();
    let mut last_emit = Instant::now();
    let mut file_done: u64 = 0;

    while remaining > 0 {
        if stop.load(Ordering::Relaxed) {
            return Err("已停止".into());
        }
        let to_read = remaining.min(buf.len() as u64) as usize;
        let n = match stream.read(&mut buf[..to_read]) {
            Ok(n) => n,
            Err(e)
                if e.kind() == io::ErrorKind::WouldBlock
                    || e.kind() == io::ErrorKind::TimedOut =>
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

        if last_emit.elapsed() >= Duration::from_millis(200) {
            let speed =
                (file_done as f64 / file_start.elapsed().as_secs_f64().max(0.001)) as u64;
            let _ = app.emit(
                EVT_PROGRESS,
                FileProgressPayload {
                    id: job_id.to_string(),
                    path: task.path.clone(),
                    done: file_done,
                    total: size,
                    speed,
                },
            );
            last_emit = Instant::now();
        }
    }

    let _ = app.emit(
        EVT_PROGRESS,
        FileProgressPayload {
            id: job_id.to_string(),
            path: task.path.clone(),
            done: size,
            total: size,
            speed: 0,
        },
    );
    let _ = done_tx.send(FileDone {
        path: task.path.clone(),
        size: task.size,
    });
    Ok(())
}

fn emitter_loop<R: Runtime>(
    app: &AppHandle<R>,
    job_id: &str,
    done_rx: Receiver<FileDone>,
    progress: Arc<Mutex<JobProgress>>,
    stop: Arc<AtomicBool>,
) {
    let mut buf: Vec<FileDone> = Vec::new();
    let mut last_files_emit = Instant::now();
    let mut last_job_emit = Instant::now();
    let mut speed_time = Instant::now();
    let mut speed_bytes: u64 = 0;

    loop {
        // A stop request must surface in the UI immediately. Report the
        // terminal state ourselves instead of waiting for the channel to
        // disconnect, which can lag behind when other threads are slow to
        // unwind (otherwise the UI would keep seeing "running" events).
        if stop.load(Ordering::Relaxed) {
            flush_files(app, job_id, &mut buf);
            let p = progress.lock().unwrap();
            let payload = job_payload(job_id, JobStatus::Stopped, &p, None);
            drop(p);
            let _ = app.emit(EVT_JOB, payload);
            break;
        }
        let recv = match done_rx.recv_timeout(Duration::from_millis(50)) {
            Ok(f) => Some(f),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => {
                flush_files(app, job_id, &mut buf);
                break;
            }
        };
        if let Some(f) = recv {
            buf.push(f);
        }
        if (last_files_emit.elapsed() >= Duration::from_millis(100) && !buf.is_empty())
            || buf.len() >= 1000
        {
            flush_files(app, job_id, &mut buf);
            last_files_emit = Instant::now();
        }
        if last_job_emit.elapsed() >= Duration::from_millis(250) {
            let mut p = progress.lock().unwrap();
            let dt = speed_time.elapsed().as_secs_f64().max(0.001);
            p.speed = ((p.done_bytes - speed_bytes) as f64 / dt) as u64;
            speed_time = Instant::now();
            speed_bytes = p.done_bytes;
            let payload = job_payload(job_id, JobStatus::Running, &p, None);
            drop(p);
            let _ = app.emit(EVT_JOB, payload);
            last_job_emit = Instant::now();
        }
    }
}

fn flush_files<R: Runtime>(app: &AppHandle<R>, job_id: &str, buf: &mut Vec<FileDone>) {
    if buf.is_empty() {
        return;
    }
    let files = std::mem::take(buf);
    let _ = app.emit(
        EVT_FILES_DONE,
        FilesDonePayload {
            id: job_id.to_string(),
            files,
        },
    );
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
        message,
    }
}

pub fn log_info<R: Runtime>(app: &AppHandle<R>, job_id: &str, message: impl Into<String>) {
    let _ = app.emit(
        EVT_LOG,
        LogPayload {
            id: job_id.to_string(),
            level: "info".into(),
            message: message.into(),
            time: now_secs(),
        },
    );
}

pub fn log_error<R: Runtime>(app: &AppHandle<R>, job_id: &str, message: impl Into<String>) {
    let _ = app.emit(
        EVT_LOG,
        LogPayload {
            id: job_id.to_string(),
            level: "error".into(),
            message: message.into(),
            time: now_secs(),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

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

        let seen: HashSet<String> = ["keep", "keep/a.txt", "keep/sub", "keep/sub/b.txt"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut deleted: Vec<String> = Vec::new();
        let (files, dirs) = delete_missing(&root, &seen, &mut deleted);

        assert_eq!(files, 1, "only stale.txt should be removed");
        assert_eq!(dirs, 1, "only stale_dir should be removed");
        assert_eq!(deleted.len(), 2);
        assert!(deleted.contains(&"stale.txt".to_string()));
        assert!(deleted.contains(&"stale_dir".to_string()));
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
