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
  /** Retry an interrupted remote scan, using a disk database to suppress duplicates. */
  rescanOnInterrupt: boolean;
  /** When enabled, also delete local files/dirs that no longer exist on the remote. */
  deleteRemoved: boolean;
}

export interface RemoteInfo {
  totalFiles: number;
  totalBytes: number;
}

export interface ServerConnectionInfo {
  id: number;
  peer: string;
  active: boolean;
  kind: "connecting" | "control" | "listing" | "transfer" | "error";
  share: string | null;
  currentFile: string | null;
  activity: string;
  bytesSent: number;
  connectedAt: number;
  lastActiveAt: number;
}

export interface ServerStatus {
  running: boolean;
  addr: string | null;
  shares: string[];
  connections: ServerConnectionInfo[];
}

export type JobStatus = "running" | "finished" | "stopped" | "error";
export type SyncPhase =
  | "preparing"
  | "scanning"
  | "transferring"
  | "retrying"
  | "finalizing"
  | "deleting"
  | "finished"
  | "stopped"
  | "error";


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
  scannedFiles: number;
  activeFiles: number;
  listingComplete: boolean;
  listAttempt: number;
  phase: SyncPhase;
  activity: string;
  currentFile: string | null;
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
  scanWorkers: number;
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
  connections?: ServerConnectionInfo[];
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
  scannedFiles: number;
  activeFiles: number;
  listingComplete: boolean;
  listAttempt: number;
  phase: SyncPhase;
  activity: string;
  currentFile: string | null;
}

export interface ActiveFileProgress {
  path: string;
  done: number;
  total: number;
  speed: number;
}

export interface FileProgressEvent {
  id: string;
  /** Complete bounded snapshot of files active at emit time. */
  files: ActiveFileProgress[];
}

export interface CompletedEntry {
  name: string;
  path: string;
  isDir: boolean;
  size: number;
  modified: number | null;
}

export interface CompletedPage {
  relative: string;
  offset: number;
  hasMore: boolean;
  entries: CompletedEntry[];
}

export interface RetryEvent {
  id: string;
  path: string;
  attempt: number;
  maxRetries: number;
  /** Seconds until the next attempt. */
  retryIn: number;
  /** "retrying" while queued, "failed" when retries are exhausted. */
  state: "retrying" | "failed" | "succeeded";
}

export interface LogEvent {
  id: string;
  source: "client" | "server";
  level: "info" | "warn" | "error";
  message: string;
  time: number;
  file?: string | null;
}

export const EVT_SERVER = "sync-server";
export const EVT_JOB = "sync-job";
export const EVT_PROGRESS = "sync-progress";
export const EVT_RETRY = "sync-retry";
export const EVT_LOG = "sync-log";
