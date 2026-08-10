// Types shared between the Tauri backend and the frontend.
// Field names must match the Rust serde output (camelCase).

export interface SyncOptions {
  remoteIp: string;
  remotePort: number;
  /** Remote shared folder path on the server (Node A). */
  share: string;
  /** Local destination folder on this machine (Node B). */
  localDir: string;
  threads: number;
  /** Max total bandwidth in MB/s. 0 = unlimited. */
  bandwidthMbps: number;
  incremental: boolean;
  /** When enabled, also delete local files/dirs that no longer exist on the remote. */
  deleteRemoved: boolean;
}

export interface RemoteInfo {
  totalFiles: number;
  totalBytes: number;
}

export interface ServerStatus {
  running: boolean;
  addr: string | null;
  shares: string[];
}

export type JobStatus = "running" | "finished" | "stopped" | "error";

export interface JobSnapshot {
  id: string;
  status: JobStatus;
  remote: string;
  share: string;
  localDir: string;
  threads: number;
  incremental: boolean;
  totalFiles: number;
  doneFiles: number;
  failedFiles: number;
  skippedFiles: number;
  totalBytes: number;
  doneBytes: number;
  speed: number;
  error: string | null;
  startedAt: number;
  finishedAt: number | null;
}

export interface ServerConfig {
  id: number;
  name: string;
  ip: string;
  port: number;
  folders: string[];
  createdAt: number;
}

export interface RecentConnection {
  id: number;
  ip: string;
  port: number;
  share: string;
  localDir: string;
  lastUsed: number;
}

// ---- Tauri events ----

export interface ServerEvent {
  running: boolean;
  addr?: string | null;
  shares?: string[];
  message?: string;
}

export interface JobEvent {
  id: string;
  status: JobStatus;
  totalFiles: number;
  doneFiles: number;
  failedFiles: number;
  skippedFiles: number;
  totalBytes: number;
  doneBytes: number;
  speed: number;
  message?: string;
}

export interface FileProgressEvent {
  id: string;
  path: string;
  done: number;
  total: number;
  speed: number;
}

export interface FilesDoneEvent {
  id: string;
  files: { path: string; size: number }[];
}

export interface FilesDeletedEvent {
  id: string;
  /** Relative paths of files/dirs deleted during mirror sync. */
  files: string[];
}

export interface RetryEvent {
  id: string;
  path: string;
  attempt: number;
  maxRetries: number;
  /** Seconds until the next attempt. */
  retryIn: number;
  /** "retrying" while queued, "failed" when retries are exhausted. */
  state: "retrying" | "failed";
}

export interface LogEvent {
  id: string;
  level: "info" | "warn" | "error";
  message: string;
  time: number;
}

export const EVT_SERVER = "sync-server";
export const EVT_JOB = "sync-job";
export const EVT_PROGRESS = "sync-progress";
export const EVT_FILES_DONE = "sync-files-done";
export const EVT_FILES_DELETED = "sync-files-deleted";
export const EVT_RETRY = "sync-retry";
export const EVT_LOG = "sync-log";
