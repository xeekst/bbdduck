//! Node A: a TCP server that shares one or more local folders.

use std::collections::{HashMap, VecDeque};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossbeam_channel::{bounded, unbounded, RecvTimeoutError, SendTimeoutError, Sender};
use tauri::AppHandle;

use super::logging;
use super::model::{now_secs, ServerConnectionInfo};
use super::protocol::{
    mtime_secs, read_msg, safe_join, write_msg, ClientMsg, FileEntry, ServerMsg, PROTOCOL_VERSION,
};
use super::{half_cpu_workers, MAX_PATH_FAILURES};

const IO_TIMEOUT: Duration = Duration::from_secs(120);
const BATCH_SIZE: usize = 500;
const CHUNK: usize = 256 * 1024;
const RECENT_CONNECTIONS: usize = 20;
/// Cap on pending file entries waiting to be streamed (backpressure).
const ENTRY_QUEUE_CAP: usize = 8192;
/// Short pause between path retries so transient filesystem races can recover.
const PATH_RETRY_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Default)]
struct ConnectionTracker {
    active: HashMap<u64, ServerConnectionInfo>,
    recent: VecDeque<ServerConnectionInfo>,
}

fn server_info(app: Option<&AppHandle>, message: impl Into<String>) {
    let message = message.into();
    if let Some(app) = app {
        logging::server_info(app, message);
    } else {
        eprintln!("[bbdduck-server] {message}");
    }
}

fn server_warn(app: Option<&AppHandle>, message: impl Into<String>) {
    let message = message.into();
    if let Some(app) = app {
        logging::server_warn(app, message);
    } else {
        eprintln!("[bbdduck-server] WARN {message}");
    }
}

fn server_error(app: Option<&AppHandle>, message: impl Into<String>) {
    let message = message.into();
    if let Some(app) = app {
        logging::server_error(app, message);
    } else {
        eprintln!("[bbdduck-server] ERROR {message}");
    }
}

pub struct ServerHandle {
    running: Arc<AtomicBool>,
    addr: Mutex<Option<String>>,
    shares: Arc<Mutex<Vec<String>>>,
    connections: Arc<Mutex<ConnectionTracker>>,
    next_connection: Arc<AtomicU64>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl ServerHandle {
    pub fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            addr: Mutex::new(None),
            shares: Arc::new(Mutex::new(Vec::new())),
            connections: Arc::new(Mutex::new(ConnectionTracker::default())),
            next_connection: Arc::new(AtomicU64::new(1)),
            thread: Mutex::new(None),
        }
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    pub fn addr(&self) -> Option<String> {
        self.addr.lock().unwrap().clone()
    }

    pub fn shares(&self) -> Vec<String> {
        self.shares.lock().unwrap().clone()
    }

    pub fn connections(&self) -> Vec<ServerConnectionInfo> {
        let tracker = self.connections.lock().unwrap();
        let mut items: Vec<_> = tracker.active.values().cloned().collect();
        items.sort_by(|a, b| b.last_active_at.cmp(&a.last_active_at));
        items.extend(tracker.recent.iter().cloned());
        items
    }

    /// Start listening on `ip:port` sharing `folders`. `scan_workers` is the
    /// number of parallel walker threads used per listing; `0` means auto
    /// (half of the logical CPUs). Returns the actual bound address.
    pub fn start(
        &self,
        ip: String,
        port: u16,
        folders: Vec<String>,
        scan_workers: usize,
    ) -> Result<String, String> {
        self.start_inner(None, ip, port, folders, scan_workers)
    }

    pub fn start_with_app(
        &self,
        app: AppHandle,
        ip: String,
        port: u16,
        folders: Vec<String>,
        scan_workers: usize,
    ) -> Result<String, String> {
        self.start_inner(Some(app), ip, port, folders, scan_workers)
    }

    fn start_inner(
        &self,
        app: Option<AppHandle>,
        ip: String,
        port: u16,
        folders: Vec<String>,
        scan_workers: usize,
    ) -> Result<String, String> {
        self.stop();

        let listener = TcpListener::bind((ip.as_str(), port))
            .map_err(|e| format!("监听 {ip}:{port} 失败: {e}"))?;
        let local = listener
            .local_addr()
            .map_err(|e| format!("获取监听地址失败: {e}"))?;
        // Non-blocking so `stop()` can join promptly.
        listener
            .set_nonblocking(true)
            .map_err(|e| format!("设置非阻塞失败: {e}"))?;

        let addr_str = local.to_string();
        self.running.store(true, Ordering::Relaxed);
        *self.addr.lock().unwrap() = Some(addr_str.clone());
        *self.shares.lock().unwrap() = folders.clone();

        let running = self.running.clone();
        let shares = self.shares.clone();
        let connections = self.connections.clone();
        let next_connection = self.next_connection.clone();
        let handle = thread::spawn(move || {
            for stream in listener.incoming() {
                if !running.load(Ordering::Relaxed) {
                    break;
                }
                match stream {
                    Ok(s) => {
                        let shares = Arc::clone(&shares);
                        let running = Arc::clone(&running);
                        let connections = Arc::clone(&connections);
                        let app = app.clone();
                        let connection_id = next_connection.fetch_add(1, Ordering::Relaxed);
                        let peer = s
                            .peer_addr()
                            .map(|addr| addr.to_string())
                            .unwrap_or_else(|_| "unknown".into());
                        let now = now_secs();
                        connections.lock().unwrap().active.insert(
                            connection_id,
                            ServerConnectionInfo {
                                id: connection_id,
                                peer,
                                active: true,
                                kind: "connecting".into(),
                                share: None,
                                current_file: None,
                                activity: "正在握手".into(),
                                bytes_sent: 0,
                                connected_at: now,
                                last_active_at: now,
                            },
                        );
                        thread::spawn(move || {
                            handle_connection(
                                s,
                                shares,
                                running,
                                scan_workers,
                                connections,
                                connection_id,
                                app,
                            )
                        });
                    }
                    Err(_) => thread::sleep(Duration::from_millis(20)),
                }
            }
        });
        *self.thread.lock().unwrap() = Some(handle);
        Ok(addr_str)
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(t) = self.thread.lock().unwrap().take() {
            let _ = t.join();
        }
        *self.addr.lock().unwrap() = None;
        let mut tracker = self.connections.lock().unwrap();
        tracker.active.clear();
        tracker.recent.clear();
    }
}

fn update_connection(
    tracker: &Arc<Mutex<ConnectionTracker>>,
    id: u64,
    kind: &str,
    share: Option<&str>,
    current_file: Option<&str>,
    activity: impl Into<String>,
) {
    if let Some(item) = tracker.lock().unwrap().active.get_mut(&id) {
        item.kind = kind.to_string();
        item.share = share.map(str::to_string);
        item.current_file = current_file.map(str::to_string);
        item.activity = activity.into();
        item.last_active_at = now_secs();
    }
}

fn add_connection_bytes(tracker: &Arc<Mutex<ConnectionTracker>>, id: u64, bytes: u64) {
    if let Some(item) = tracker.lock().unwrap().active.get_mut(&id) {
        item.bytes_sent = item.bytes_sent.saturating_add(bytes);
        item.last_active_at = now_secs();
    }
}

struct ConnectionGuard {
    tracker: Arc<Mutex<ConnectionTracker>>,
    id: u64,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        let mut tracker = self.tracker.lock().unwrap();
        if let Some(mut item) = tracker.active.remove(&self.id) {
            item.active = false;
            item.last_active_at = now_secs();
            item.activity = if item.kind == "transfer" {
                "文件连接已结束".into()
            } else {
                "连接已结束".into()
            };
            tracker.recent.push_front(item);
            tracker.recent.truncate(RECENT_CONNECTIONS);
        }
    }
}

fn handle_connection(
    mut stream: TcpStream,
    shares: Arc<Mutex<Vec<String>>>,
    _running: Arc<AtomicBool>,
    scan_workers: usize,
    connections: Arc<Mutex<ConnectionTracker>>,
    connection_id: u64,
    app: Option<AppHandle>,
) {
    let _connection_guard = ConnectionGuard {
        tracker: Arc::clone(&connections),
        id: connection_id,
    };

    // On Windows, sockets accepted from a non-blocking listener inherit the
    // non-blocking flag; force blocking so our framed reads work.
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));

    // Handshake
    match read_msg::<_, ClientMsg>(&mut stream) {
        Ok(Some(ClientMsg::Hello { version })) if version == PROTOCOL_VERSION => {
            let _ = write_msg(
                &mut stream,
                &ServerMsg::HelloAck {
                    version: PROTOCOL_VERSION,
                    name: "bbdduck".into(),
                },
            );
        }
        Ok(Some(_)) => {
            let _ = write_msg(
                &mut stream,
                &ServerMsg::Error {
                    message: "协议版本不匹配".into(),
                },
            );
            return;
        }
        _ => return,
    }
    update_connection(
        &connections,
        connection_id,
        "control",
        None,
        None,
        "握手完成，正在等待请求",
    );

    loop {
        let msg = match read_msg::<_, ClientMsg>(&mut stream) {
            Ok(Some(m)) => m,
            Ok(None) | Err(_) => break,
        };

        let is_fetch = matches!(msg, ClientMsg::FetchFile { .. });
        let result = match &msg {
            ClientMsg::ListShares => {
                update_connection(
                    &connections,
                    connection_id,
                    "control",
                    None,
                    None,
                    "正在读取共享目录列表",
                );
                let list = shares.lock().unwrap().clone();
                write_msg(&mut stream, &ServerMsg::Shares { shares: list })
            }
            ClientMsg::ListFiles { share } => {
                update_connection(
                    &connections,
                    connection_id,
                    "listing",
                    Some(share),
                    None,
                    format!("正在扫描共享目录：{share}"),
                );
                if !shares.lock().unwrap().contains(share) {
                    server_error(app.as_ref(), format!("请求的共享文件夹不存在：{share}"));
                    write_msg(
                        &mut stream,
                        &ServerMsg::Error {
                            message: format!("共享文件夹不存在: {share}"),
                        },
                    )
                } else {
                    serve_file_list(
                        app.as_ref(),
                        &mut stream,
                        PathBuf::from(share),
                        scan_workers,
                        &connections,
                        connection_id,
                    )
                }
            }
            ClientMsg::FetchFile { share, path } => {
                update_connection(
                    &connections,
                    connection_id,
                    "transfer",
                    Some(share),
                    Some(path),
                    format!("正在发送文件：{path}"),
                );
                if !shares.lock().unwrap().contains(share) {
                    server_error(app.as_ref(), format!("请求的共享文件夹不存在：{share}"));
                    write_msg(
                        &mut stream,
                        &ServerMsg::Error {
                            message: format!("共享文件夹不存在: {share}"),
                        },
                    )
                } else {
                    serve_file(
                        app.as_ref(),
                        &mut stream,
                        PathBuf::from(share),
                        path,
                        &connections,
                        connection_id,
                    )
                }
            }
            ClientMsg::Hello { .. } => write_msg(
                &mut stream,
                &ServerMsg::Error {
                    message: "重复握手".into(),
                },
            ),
        };

        if let Err(error) = result {
            let message = format!("连接 #{} 处理失败：{}", connection_id, error);
            update_connection(
                &connections,
                connection_id,
                "error",
                None,
                None,
                message.clone(),
            );
            server_error(app.as_ref(), message);
            break;
        }
        if is_fetch {
            // One file per connection: close after the raw payload.
            break;
        }
    }
}

/// Streams the file tree of `root` in batches. A small pool of worker threads
/// walks directories in parallel (order is not significant to the client,
/// which treats entries as an unordered set), so `metadata()` syscalls run in
/// parallel on multi-core machines. Only files count toward totals.
fn serve_file_list(
    app: Option<&AppHandle>,
    stream: &mut TcpStream,
    root: PathBuf,
    scan_workers: usize,
    connections: &Arc<Mutex<ConnectionTracker>>,
    connection_id: u64,
) -> io::Result<()> {
    let share_display = root.to_string_lossy().into_owned();
    server_info(app, format!("开始扫描共享目录：{share_display}"));
    // Directory queue is unbounded on purpose: walkers both produce and
    // consume it, so a bounded queue could deadlock (all walkers blocked in
    // send while none is left to receive). Directories are small and each is
    // processed exactly once, so this stays bounded by the total dir count.
    let (dir_tx, dir_rx) = unbounded::<PathBuf>();
    let (entry_tx, entry_rx) = bounded::<FileEntry>(ENTRY_QUEUE_CAP);
    // Directories either queued or being processed; starts at 1 for the root.
    let pending = Arc::new(AtomicU64::new(1));
    let skipped_paths = Arc::new(AtomicU64::new(0));
    let stop = Arc::new(AtomicBool::new(false));

    if dir_tx.send(root.clone()).is_err() {
        return Ok(());
    }

    let mut workers = Vec::new();
    // 0 = auto (half of the logical CPUs).
    let scan_workers = if scan_workers == 0 {
        half_cpu_workers()
    } else {
        scan_workers
    };
    for _ in 0..scan_workers {
        let app = app.cloned();
        let dir_rx = dir_rx.clone();
        let entry_tx = entry_tx.clone();
        let dir_tx = dir_tx.clone();
        let pending = Arc::clone(&pending);
        let skipped_paths = Arc::clone(&skipped_paths);
        let stop = Arc::clone(&stop);
        let root = root.clone();
        workers.push(thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let dir = match dir_rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(d) => d,
                    Err(RecvTimeoutError::Timeout) => {
                        if pending.load(Ordering::Relaxed) == 0 {
                            break;
                        }
                        continue;
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                };
                walk_dir(
                    app.as_ref(),
                    &root,
                    &dir,
                    &dir_tx,
                    &entry_tx,
                    &pending,
                    &skipped_paths,
                    &stop,
                );
                pending.fetch_sub(1, Ordering::AcqRel);
            }
        }));
    }
    drop(dir_tx);
    drop(entry_tx);

    // Drainer: stream batches to the client as entries arrive.
    let mut batch: Vec<FileEntry> = Vec::new();
    let mut total = 0u64;
    let mut total_bytes = 0u64;
    let mut drain_err: Option<io::Error> = None;
    loop {
        match entry_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(e) => {
                if !e.is_dir {
                    total += 1;
                    total_bytes += e.size;
                    if total % 10_000 == 0 {
                        update_connection(
                            connections,
                            connection_id,
                            "listing",
                            Some(&share_display),
                            None,
                            format!("正在扫描目录：已发现 {total} 个文件"),
                        );
                    }
                    if total % 100_000 == 0 {
                        server_info(
                            app,
                            format!(
                                "共享目录扫描进度：{share_display}，已发现 {total} 个文件，{total_bytes} 字节"
                            ),
                        );
                    }
                }
                batch.push(e);
            }
            Err(RecvTimeoutError::Timeout) => {
                // Keep the stream moving even if walkers are momentarily slow.
                if !batch.is_empty() {
                    if let Err(e) = write_msg(
                        stream,
                        &ServerMsg::FileEntries {
                            entries: std::mem::take(&mut batch),
                        },
                    ) {
                        server_error(app, format!("发送目录扫描结果失败：{e}"));
                        drain_err = Some(e);
                        break;
                    }
                }
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if batch.len() >= BATCH_SIZE {
            if let Err(e) = write_msg(
                stream,
                &ServerMsg::FileEntries {
                    entries: std::mem::take(&mut batch),
                },
            ) {
                server_error(app, format!("发送目录扫描结果失败：{e}"));
                drain_err = Some(e);
                break;
            }
        }
    }
    if !batch.is_empty() {
        if let Err(e) = write_msg(stream, &ServerMsg::FileEntries { entries: batch }) {
            drain_err = Some(e);
        }
    }

    // Always stop walkers before joining so no worker stays blocked on send.
    stop.store(true, Ordering::Relaxed);
    for w in workers {
        let _ = w.join();
    }
    if let Some(e) = drain_err {
        return Err(e);
    }
    let skipped_paths = skipped_paths.load(Ordering::Relaxed);
    update_connection(
        connections,
        connection_id,
        "listing",
        Some(&share_display),
        None,
        format!("目录扫描完成：{total} 个文件，跳过 {skipped_paths} 个异常路径"),
    );
    let summary = format!(
        "共享目录扫描完成：{share_display}，{total} 个文件，{total_bytes} 字节，跳过 {skipped_paths} 个异常路径"
    );
    if skipped_paths > 0 {
        server_warn(app, summary);
    } else {
        server_info(app, summary);
    }
    write_msg(
        stream,
        &ServerMsg::FileEntriesEnd {
            total,
            total_bytes,
            skipped_paths,
        },
    )
}

/// Scan one directory: stat each entry and forward it; enqueue subdirectories
/// for other workers. Each directory is processed by exactly one worker.
fn walk_dir(
    app: Option<&AppHandle>,
    root: &Path,
    dir: &Path,
    dir_tx: &Sender<PathBuf>,
    entry_tx: &Sender<FileEntry>,
    pending: &AtomicU64,
    skipped_paths: &AtomicU64,
    stop: &AtomicBool,
) {
    let entries = match retry_path_operation(app, dir, "读取目录", stop, || fs::read_dir(dir)) {
        Ok(entries) => entries,
        Err(error) => {
            if !stop.load(Ordering::Relaxed) {
                record_skipped_path(app, dir, "读取目录", &error, skipped_paths);
            }
            return;
        }
    };

    for entry_result in entries {
        let entry = match entry_result {
            Ok(entry) => entry,
            Err(error) => {
                skipped_paths.fetch_add(1, Ordering::Relaxed);
                server_error(
                    app,
                    format!(
                        "读取目录项失败，无法确定 {dir_display} 下的具体子路径，已跳过该目录项并继续扫描：{error}",
                        dir_display = dir.display()
                    ),
                );
                continue;
            }
        };
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let path = entry.path();
        let md =
            match retry_path_operation(app, &path, "读取路径信息", stop, || entry.metadata())
            {
                Ok(metadata) => metadata,
                Err(error) => {
                    if !stop.load(Ordering::Relaxed) {
                        record_skipped_path(app, &path, "读取路径信息", &error, skipped_paths);
                    }
                    continue;
                }
            };
        let rel = path
            .strip_prefix(root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        if md.is_dir() {
            pending.fetch_add(1, Ordering::AcqRel);
            // Unbounded channel: never blocks (see serve_file_list).
            if dir_tx.send(path).is_err() {
                pending.fetch_sub(1, Ordering::AcqRel);
                continue;
            }
            send_with_stop(
                entry_tx,
                FileEntry {
                    path: rel,
                    size: 0,
                    mtime: mtime_secs(&md),
                    is_dir: true,
                },
                stop,
            );
        } else {
            send_with_stop(
                entry_tx,
                FileEntry {
                    path: rel,
                    size: md.len(),
                    mtime: mtime_secs(&md),
                    is_dir: false,
                },
                stop,
            );
        }
    }
}

/// Retry one filesystem operation for a concrete path. The error remains local
/// to that path; callers record and skip it after the retry budget is exhausted.
fn retry_path_operation<T>(
    app: Option<&AppHandle>,
    path: &Path,
    operation: &str,
    stop: &AtomicBool,
    mut action: impl FnMut() -> io::Result<T>,
) -> io::Result<T> {
    let mut last_error: Option<io::Error> = None;
    for failure in 1..=MAX_PATH_FAILURES {
        if stop.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "扫描已停止"));
        }
        match action() {
            Ok(value) => {
                if failure > 1 {
                    server_info(
                        app,
                        format!(
                            "{operation}恢复：{path}（第 {failure} 次尝试成功）",
                            path = path.display()
                        ),
                    );
                }
                return Ok(value);
            }
            Err(error) => {
                if failure < MAX_PATH_FAILURES && (failure == 1 || failure % 10 == 0) {
                    server_warn(
                        app,
                        format!(
                            "{operation}失败：{path}：{error}（失败 {failure}/{MAX_PATH_FAILURES} 次，将继续重试）",
                            path = path.display()
                        ),
                    );
                }
                last_error = Some(error);
                if failure < MAX_PATH_FAILURES {
                    thread::sleep(PATH_RETRY_INTERVAL);
                }
            }
        }
    }
    Err(last_error.unwrap_or_else(|| io::Error::other("路径操作失败")))
}

fn record_skipped_path(
    app: Option<&AppHandle>,
    path: &Path,
    operation: &str,
    error: &io::Error,
    skipped_paths: &AtomicU64,
) {
    skipped_paths.fetch_add(1, Ordering::Relaxed);
    server_error(
        app,
        format!(
            "{operation}连续失败 {MAX_PATH_FAILURES} 次，已跳过该路径并继续扫描：{path}：{error}",
            path = path.display()
        ),
    );
}

/// Send a value on a bounded channel without blocking forever: periodically
/// re-check `stop` so an aborted listing can shut down its workers promptly.
fn send_with_stop<T>(tx: &Sender<T>, value: T, stop: &AtomicBool) {
    let mut value = value;
    loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        match tx.send_timeout(value, Duration::from_millis(50)) {
            Ok(()) => return,
            Err(SendTimeoutError::Timeout(item)) => value = item,
            Err(SendTimeoutError::Disconnected(_)) => return,
        }
    }
}

fn serve_file(
    app: Option<&AppHandle>,
    stream: &mut TcpStream,
    root: PathBuf,
    rel: &str,
    connections: &Arc<Mutex<ConnectionTracker>>,
    connection_id: u64,
) -> io::Result<()> {
    let path = match safe_join(&root, rel) {
        Some(p) => p,
        None => {
            server_error(app, format!("拒绝非法文件路径：{rel}"));
            write_msg(
                stream,
                &ServerMsg::Error {
                    message: "非法路径".into(),
                },
            )?;
            return Ok(());
        }
    };
    let mut file = match File::open(&path) {
        Ok(f) => f,
        Err(e) => {
            server_error(app, format!("无法打开文件 {rel}：{e}"));
            write_msg(
                stream,
                &ServerMsg::Error {
                    message: format!("无法打开文件 {rel}: {e}"),
                },
            )?;
            return Ok(());
        }
    };
    let md = match file.metadata() {
        Ok(m) => m,
        Err(e) => {
            server_error(app, format!("读取文件信息失败 {rel}：{e}"));
            write_msg(
                stream,
                &ServerMsg::Error {
                    message: format!("读取文件信息失败: {e}"),
                },
            )?;
            return Ok(());
        }
    };
    write_msg(
        stream,
        &ServerMsg::FileMeta {
            size: md.len(),
            mtime: mtime_secs(&md),
        },
    )?;
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = match file.read(&mut buf) {
            Ok(n) => n,
            Err(e) => return Err(e),
        };
        if n == 0 {
            break;
        }
        stream.write_all(&buf[..n])?;
        add_connection_bytes(connections, connection_id, n as u64);
    }
    update_connection(
        connections,
        connection_id,
        "transfer",
        Some(&root.to_string_lossy()),
        Some(rel),
        format!("文件发送完成：{rel}"),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_directory_is_skipped_without_stopping_the_scan() {
        let missing =
            std::env::temp_dir().join(format!("bbdduck-missing-scan-path-{}", std::process::id()));
        let _ = fs::remove_dir_all(&missing);
        let (dir_tx, _dir_rx) = unbounded::<PathBuf>();
        let (entry_tx, entry_rx) = bounded::<FileEntry>(8);
        let pending = AtomicU64::new(1);
        let skipped_paths = AtomicU64::new(0);
        let stop = AtomicBool::new(false);

        walk_dir(
            None,
            &missing,
            &missing,
            &dir_tx,
            &entry_tx,
            &pending,
            &skipped_paths,
            &stop,
        );

        assert_eq!(skipped_paths.load(Ordering::Relaxed), 1);
        assert!(!stop.load(Ordering::Relaxed));
        assert!(entry_rx.is_empty());
    }
}
