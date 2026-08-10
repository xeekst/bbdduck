//! Node A: a TCP server that shares one or more local folders.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::protocol::{
    mtime_secs, read_msg, safe_join, write_msg, ClientMsg, FileEntry, ServerMsg, PROTOCOL_VERSION,
};

const IO_TIMEOUT: Duration = Duration::from_secs(120);
const BATCH_SIZE: usize = 500;
const CHUNK: usize = 256 * 1024;

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

    /// Start listening on `ip:port` sharing `folders`. Returns the actual bound address.
    pub fn start(&self, ip: String, port: u16, folders: Vec<String>) -> Result<String, String> {
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
                        thread::spawn(move || handle_connection(s, shares, running));
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
                    serve_file_list(&mut stream, PathBuf::from(share))
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

/// Streams the file tree of `root` in batches. Only files count toward totals.
fn serve_file_list(stream: &mut TcpStream, root: PathBuf) -> io::Result<()> {
    let mut batch: Vec<FileEntry> = Vec::new();
    let (total, total_bytes) = walk_and_send(&root, &root, stream, &mut batch)?;
    if !batch.is_empty() {
        write_msg(stream, &ServerMsg::FileEntries { entries: batch })?;
    }
    write_msg(
        stream,
        &ServerMsg::FileEntriesEnd {
            total,
            total_bytes,
        },
    )
}

fn walk_and_send(
    root: &Path,
    dir: &Path,
    stream: &mut TcpStream,
    batch: &mut Vec<FileEntry>,
) -> io::Result<(u64, u64)> {
    let mut total = 0u64;
    let mut total_bytes = 0u64;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
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
            batch.push(FileEntry {
                path: rel,
                size: 0,
                mtime: mtime_secs(&md),
                is_dir: true,
            });
            let (t, b) = walk_and_send(root, &entry.path(), stream, batch)?;
            total += t;
            total_bytes += b;
        } else {
            batch.push(FileEntry {
                path: rel,
                size: md.len(),
                mtime: mtime_secs(&md),
                is_dir: false,
            });
            total += 1;
            total_bytes += md.len();
        }
        if batch.len() >= BATCH_SIZE {
            write_msg(stream, &ServerMsg::FileEntries {
                entries: std::mem::take(batch),
            })?;
        }
    }
    Ok((total, total_bytes))
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
