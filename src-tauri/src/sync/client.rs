//! Node B client helpers: share listing, remote file listing, and a shared
//! bandwidth limiter used by the download workers.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::protocol::{
    connect_with_timeout, read_msg, write_msg, ClientMsg, FileEntry, ServerMsg, PROTOCOL_VERSION,
};

/// Connect, handshake and list the shared folders of a Node A server.
pub fn list_shares(ip: &str, port: u16) -> Result<Vec<String>, String> {
    let mut stream = connect_with_timeout(ip, port, 5)?;
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .map_err(|e| e.to_string())?;
    write_msg(&mut stream, &ClientMsg::Hello { version: PROTOCOL_VERSION })
        .map_err(|e| e.to_string())?;
    match read_msg::<_, ServerMsg>(&mut stream).map_err(|e| e.to_string())? {
        Some(ServerMsg::HelloAck { .. }) => {}
        Some(ServerMsg::Error { message }) => return Err(message),
        _ => return Err("服务器响应异常".into()),
    }
    write_msg(&mut stream, &ClientMsg::ListShares).map_err(|e| e.to_string())?;
    match read_msg::<_, ServerMsg>(&mut stream).map_err(|e| e.to_string())? {
        Some(ServerMsg::Shares { shares }) => Ok(shares),
        Some(ServerMsg::Error { message }) => Err(message),
        _ => Err("服务器响应异常".into()),
    }
}

/// Streams the file listing of a remote share. `on_entry` returns `false` to
/// abort the scan early. Returns `(total_files, total_bytes)`.
pub fn list_remote_files(
    ip: &str,
    port: u16,
    share: &str,
    mut on_entry: impl FnMut(&FileEntry) -> bool,
) -> Result<(u64, u64), String> {
    let mut stream = connect_with_timeout(ip, port, 5)?;
    stream
        .set_read_timeout(Some(Duration::from_secs(300)))
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
        &ClientMsg::ListFiles {
            share: share.to_string(),
        },
    )
    .map_err(|e| e.to_string())?;

    loop {
        let msg = read_msg::<_, ServerMsg>(&mut stream).map_err(|e| e.to_string())?;
        match msg {
            Some(ServerMsg::FileEntries { entries }) => {
                for e in entries {
                    if !on_entry(&e) {
                        return Ok((0, 0));
                    }
                }
            }
            Some(ServerMsg::FileEntriesEnd { total, total_bytes }) => {
                return Ok((total, total_bytes));
            }
            Some(ServerMsg::Error { message }) => return Err(message),
            _ => return Err("服务器响应异常".into()),
        }
    }
}

/// Global token-bucket style limiter shared by all workers of one job.
/// `rate == 0` means unlimited.
pub struct BandwidthLimiter {
    rate: AtomicU64,
    window_start: Mutex<Instant>,
    window_bytes: AtomicU64,
}

impl BandwidthLimiter {
    pub fn new(bytes_per_sec: u64) -> Self {
        Self {
            rate: AtomicU64::new(bytes_per_sec),
            window_start: Mutex::new(Instant::now()),
            window_bytes: AtomicU64::new(0),
        }
    }

    /// Blocks until `bytes` may be transferred within the current 1s budget.
    pub fn acquire(&self, bytes: u64) {
        let rate = self.rate.load(Ordering::Relaxed);
        if rate == 0 || bytes == 0 {
            return;
        }
        loop {
            {
                let mut start = self.window_start.lock().unwrap();
                if start.elapsed().as_secs() >= 1 {
                    *start = Instant::now();
                    self.window_bytes.store(0, Ordering::Relaxed);
                }
                let used = self.window_bytes.load(Ordering::Relaxed);
                if used.saturating_add(bytes) <= rate {
                    self.window_bytes.fetch_add(bytes, Ordering::Relaxed);
                    return;
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
