//! Node A: a TCP server that shares one or more local folders.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossbeam_channel::{bounded, unbounded, RecvTimeoutError, SendTimeoutError, Sender};

use super::half_cpu_workers;
use super::protocol::{
    mtime_secs, read_msg, safe_join, write_msg, ClientMsg, FileEntry, ServerMsg, PROTOCOL_VERSION,
};

const IO_TIMEOUT: Duration = Duration::from_secs(120);
const BATCH_SIZE: usize = 500;
const CHUNK: usize = 256 * 1024;
/// Cap on pending file entries waiting to be streamed (backpressure).
const ENTRY_QUEUE_CAP: usize = 8192;

pub struct ServerHandle {
    running: Arc<AtomicBool>,
    addr: Mutex<Option<String>>,
    shares: Arc<Mutex<Vec<String>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl ServerHandle {
    pub fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            addr: Mutex::new(None),
            shares: Arc::new(Mutex::new(Vec::new())),
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
        self.stop();

        let listener =
            TcpListener::bind((ip.as_str(), port)).map_err(|e| format!("监听 {ip}:{port} 失败: {e}"))?;
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
        let handle = thread::spawn(move || {
            for stream in listener.incoming() {
                if !running.load(Ordering::Relaxed) {
                    break;
                }
                match stream {
                    Ok(s) => {
                        let shares = Arc::clone(&shares);
                        let running = Arc::clone(&running);
                        thread::spawn(move || handle_connection(s, shares, running, scan_workers));
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
    }
}

fn handle_connection(
    mut stream: TcpStream,
    shares: Arc<Mutex<Vec<String>>>,
    _running: Arc<AtomicBool>,
    scan_workers: usize,
) {
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

    loop {
        let msg = match read_msg::<_, ClientMsg>(&mut stream) {
            Ok(Some(m)) => m,
            Ok(None) | Err(_) => break,
        };

        let is_fetch = matches!(msg, ClientMsg::FetchFile { .. });
        let result = match &msg {
            ClientMsg::ListShares => {
                let list = shares.lock().unwrap().clone();
                write_msg(&mut stream, &ServerMsg::Shares { shares: list })
            }
            ClientMsg::ListFiles { share } => {
                if !shares.lock().unwrap().contains(share) {
                    write_msg(
                        &mut stream,
                        &ServerMsg::Error {
                            message: format!("共享文件夹不存在: {share}"),
                        },
                    )
                } else {
                    serve_file_list(&mut stream, PathBuf::from(share), scan_workers)
                }
            }
            ClientMsg::FetchFile { share, path } => {
                if !shares.lock().unwrap().contains(share) {
                    write_msg(
                        &mut stream,
                        &ServerMsg::Error {
                            message: format!("共享文件夹不存在: {share}"),
                        },
                    )
                } else {
                    serve_file(&mut stream, PathBuf::from(share), path)
                }
            }
            ClientMsg::Hello { .. } => write_msg(
                &mut stream,
                &ServerMsg::Error {
                    message: "重复握手".into(),
                },
            ),
        };

        if result.is_err() {
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
fn serve_file_list(stream: &mut TcpStream, root: PathBuf, scan_workers: usize) -> io::Result<()> {
    server_log(&format!("scan start: {}", root.display()));
    // Directory queue is unbounded on purpose: walkers both produce and
    // consume it, so a bounded queue could deadlock (all walkers blocked in
    // send while none is left to receive). Directories are small and each is
    // processed exactly once, so this stays bounded by the total dir count.
    let (dir_tx, dir_rx) = unbounded::<PathBuf>();
    let (entry_tx, entry_rx) = bounded::<FileEntry>(ENTRY_QUEUE_CAP);
    // Directories either queued or being processed; starts at 1 for the root.
    let pending = Arc::new(AtomicU64::new(1));
    let scan_err: Arc<Mutex<Option<io::Error>>> = Arc::new(Mutex::new(None));
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
        let dir_rx = dir_rx.clone();
        let entry_tx = entry_tx.clone();
        let dir_tx = dir_tx.clone();
        let pending = Arc::clone(&pending);
        let scan_err = Arc::clone(&scan_err);
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
                if let Err(e) = walk_dir(&root, &dir, &dir_tx, &entry_tx, &pending, &stop) {
                    server_log(&format!("scan walk error: {e}"));
                    let mut slot = scan_err.lock().unwrap();
                    if slot.is_none() {
                        *slot = Some(e);
                    }
                    stop.store(true, Ordering::Relaxed);
                    break;
                }
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
                        server_log(&format!("scan send error: {e}"));
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
                server_log(&format!("scan send error: {e}"));
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
    if let Some(e) = drain_err.or_else(|| scan_err.lock().unwrap().take()) {
        server_log(&format!("scan failed: {e}"));
        return Err(e);
    }
    server_log(&format!("scan done: {total} files, {total_bytes} bytes"));
    write_msg(
        stream,
        &ServerMsg::FileEntriesEnd {
            total,
            total_bytes,
        },
    )
}

/// Scan one directory: stat each entry and forward it; enqueue subdirectories
/// for other workers. Each directory is processed by exactly one worker.
fn walk_dir(
    root: &Path,
    dir: &Path,
    dir_tx: &Sender<PathBuf>,
    entry_tx: &Sender<FileEntry>,
    pending: &AtomicU64,
    stop: &AtomicBool,
) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        let md = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let rel = entry
            .path()
            .strip_prefix(root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        if md.is_dir() {
            pending.fetch_add(1, Ordering::AcqRel);
            // Unbounded channel: never blocks (see serve_file_list).
            let _ = dir_tx.send(entry.path());
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
    Ok(())
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

/// Diagnostic log for server-side scan issues: stderr + a temp file so it is
/// visible even when the app runs as a GUI without a console.
fn server_log(msg: &str) {
    eprintln!("[bbdduck-server] {msg}");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(std::env::temp_dir().join("bbdduck-server.log"))
    {
        let _ = f.write_all(format!("[bbdduck-server] {msg}\n").as_bytes());
    }
}

fn serve_file(stream: &mut TcpStream, root: PathBuf, rel: &str) -> io::Result<()> {
    let path = match safe_join(&root, rel) {
        Some(p) => p,
        None => {
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
    }
    Ok(())
}
