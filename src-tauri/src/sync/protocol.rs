//! Length-prefixed JSON framing protocol used between Node A (server) and Node B (client).
//!
//! Every message is `u32 LE length` followed by that many bytes of JSON.
//! File payloads are streamed as raw bytes after a `FileMeta` header on the same
//! connection (connection is closed after the payload).

use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    Hello { version: u32 },
    ListShares,
    ListFiles { share: String },
    FetchFile { share: String, path: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    HelloAck {
        version: u32,
        name: String,
    },
    Shares {
        shares: Vec<String>,
    },
    FileEntries {
        entries: Vec<FileEntry>,
    },
    FileEntriesEnd {
        total: u64,
        total_bytes: u64,
        #[serde(default)]
        skipped_paths: u64,
    },
    FileMeta {
        size: u64,
        mtime: i64,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    /// Path relative to the shared folder root, using `/` separators.
    pub path: String,
    pub size: u64,
    /// Unix seconds.
    pub mtime: i64,
    pub is_dir: bool,
}

const MAX_FRAME: usize = 64 * 1024 * 1024;

pub fn write_msg<W: Write, T: Serialize>(w: &mut W, msg: &T) -> io::Result<()> {
    let bytes = serde_json::to_vec(msg).map_err(io::Error::other)?;
    w.write_all(&(bytes.len() as u32).to_le_bytes())?;
    w.write_all(&bytes)?;
    Ok(())
}

pub fn read_msg<R: Read, T: for<'de> Deserialize<'de>>(r: &mut R) -> io::Result<Option<T>> {
    let mut len_buf = [0u8; 4];
    match r.read_exact(&mut len_buf) {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::other("message too large"));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    serde_json::from_slice(&buf)
        .map(Some)
        .map_err(io::Error::other)
}

/// Resolve a relative path against a root, rejecting anything that would
/// escape the root (absolute paths, `..`, drive prefixes, etc.).
pub fn safe_join(root: &std::path::Path, rel: &str) -> Option<std::path::PathBuf> {
    use std::path::Component;
    if rel.is_empty() {
        return Some(root.to_path_buf());
    }
    let rel_path = std::path::Path::new(rel);
    if rel_path.is_absolute() {
        return None;
    }
    let mut out = root.to_path_buf();
    for comp in rel_path.components() {
        match comp {
            Component::Normal(c) => out.push(c),
            _ => return None,
        }
    }
    Some(out)
}

/// Best-effort mtime of a metadata in unix seconds.
pub fn mtime_secs(md: &std::fs::Metadata) -> i64 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn connect_with_timeout(ip: &str, port: u16, secs: u64) -> Result<std::net::TcpStream, String> {
    use std::net::ToSocketAddrs;
    let addrs = (ip, port)
        .to_socket_addrs()
        .map_err(|e| format!("无法解析地址 {ip}:{port}: {e}"))?;
    let mut last_err: Option<String> = None;
    for addr in addrs {
        match std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_secs(secs)) {
            Ok(s) => return Ok(s),
            Err(e) => last_err = Some(e.to_string()),
        }
    }
    Err(format!(
        "连接 {ip}:{port} 失败: {}",
        last_err.unwrap_or_else(|| "无可用地址".into())
    ))
}
