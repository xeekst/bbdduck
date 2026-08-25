use serde::{Deserialize, Serialize};

/// Options for a sync job, provided by the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncOptions {
    pub remote_ip: String,
    pub remote_port: u16,
    /// Remote shared folder path on the server (Node A).
    pub share: String,
    /// Local destination folder on this machine (Node B).
    pub local_dir: String,
    pub threads: usize,
    /// Max total bandwidth in MB/s. 0 = unlimited.
    pub bandwidth_mbps: u64,
    pub incremental: bool,
    /// When enabled, also delete local files/dirs that no longer exist on the remote.
    #[serde(default)]
    pub delete_removed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteInfo {
    pub total_files: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerStatus {
    pub running: bool,
    pub addr: Option<String>,
    pub shares: Vec<String>,
    pub connections: Vec<ServerConnectionInfo>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerConnectionInfo {
    pub id: u64,
    pub peer: String,
    pub active: bool,
    /// connecting / control / listing / transfer
    pub kind: String,
    pub share: Option<String>,
    pub current_file: Option<String>,
    pub activity: String,
    pub bytes_sent: u64,
    pub connected_at: i64,
    pub last_active_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Running,
    Finished,
    Stopped,
    Error,
}

impl JobStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            JobStatus::Running => "running",
            JobStatus::Finished => "finished",
            JobStatus::Stopped => "stopped",
            JobStatus::Error => "error",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobSnapshot {
    pub id: String,
    pub status: JobStatus,
    pub remote: String,
    pub share: String,
    pub local_dir: String,
    pub threads: usize,
    pub incremental: bool,
    pub total_files: u64,
    pub done_files: u64,
    pub failed_files: u64,
    pub skipped_files: u64,
    pub total_bytes: u64,
    pub done_bytes: u64,
    pub speed: u64,
    pub scanned_files: u64,
    pub active_files: u64,
    pub listing_complete: bool,
    pub list_attempt: u32,
    pub phase: String,
    pub activity: String,
    pub current_file: Option<String>,
    pub error: Option<String>,
    pub started_at: i64,
    pub finished_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfig {
    pub id: i64,
    pub name: String,
    pub ip: String,
    pub port: u16,
    pub folders: Vec<String>,
    pub scan_workers: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentConnection {
    pub id: i64,
    pub ip: String,
    pub port: u16,
    pub share: String,
    pub local_dir: String,
    pub last_used: i64,
}

// ---------- event payloads ----------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerEventPayload {
    pub running: bool,
    pub addr: Option<String>,
    pub shares: Vec<String>,
    pub connections: Vec<ServerConnectionInfo>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobEventPayload {
    pub id: String,
    pub status: JobStatus,
    pub total_files: u64,
    pub done_files: u64,
    pub failed_files: u64,
    pub skipped_files: u64,
    pub total_bytes: u64,
    pub done_bytes: u64,
    pub speed: u64,
    pub scanned_files: u64,
    pub active_files: u64,
    pub listing_complete: bool,
    pub list_attempt: u32,
    pub phase: String,
    pub activity: String,
    pub current_file: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileProgressPayload {
    pub id: String,
    pub path: String,
    pub done: u64,
    pub total: u64,
    pub speed: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDone {
    pub path: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilesDonePayload {
    pub id: String,
    pub files: Vec<FileDone>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilesDeletedPayload {
    pub id: String,
    /// Relative paths of deleted files/dirs.
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetryPayload {
    pub id: String,
    pub path: String,
    pub attempt: u32,
    pub max_retries: u32,
    /// Seconds until the next attempt.
    pub retry_in: u64,
    /// "retrying" while queued, "failed" when retries are exhausted.
    pub state: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogPayload {
    pub id: String,
    /// client / server
    pub source: String,
    pub level: String,
    pub message: String,
    pub time: i64,
    pub file: Option<String>,
}

pub const EVT_SERVER: &str = "sync-server";
pub const EVT_JOB: &str = "sync-job";
pub const EVT_PROGRESS: &str = "sync-progress";
pub const EVT_FILES_DONE: &str = "sync-files-done";
pub const EVT_FILES_DELETED: &str = "sync-files-deleted";
pub const EVT_RETRY: &str = "sync-retry";
pub const EVT_LOG: &str = "sync-log";

pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
