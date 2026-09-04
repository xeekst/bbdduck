import type {
  ActiveFileProgress,
  JobEvent,
  LogEvent,
  RetryEvent,
} from "./sync-types";

/** UI state is bounded independently of the number of files in a sync job. */
export const MAX_LOGS = 5000;
export const MAX_RETRY_ITEMS = 10000;

export interface TransferRow extends ActiveFileProgress {
  key: string;
  status: "active";
}

export interface LogEntry {
  time: number;
  source: "client" | "server";
  level: "info" | "warn" | "error";
  message: string;
  file?: string | null;
}

export interface RetryItem {
  path: string;
  attempt: number;
  maxRetries: number;
  /** Timestamp (ms) when the next attempt starts. */
  retryAt: number;
}

class SyncStore {
  version = 0;
  private listeners = new Set<() => void>();
  private notifyTimer: ReturnType<typeof setTimeout> | null = null;

  jobId: string | null = null;
  /** Complete snapshot of currently active files; bounded by worker count. */
  rows: TransferRow[] = [];
  logs: LogEntry[] = [];
  job: JobEvent | null = null;
  activeCount = 0;
  retries = new Map<string, RetryItem>();
  share: string | null = null;
  localDir: string | null = null;
  startedAt: number | null = null;
  finishedAt: number | null = null;

  subscribe = (fn: () => void) => {
    this.listeners.add(fn);
    return () => {
      this.listeners.delete(fn);
    };
  };

  getSnapshot = () => this.version;

  private bump() {
    // Backend snapshots arrive at most twice per second. Keep a small coalescing
    // window for job/log events that happen at the same time.
    if (this.notifyTimer) return;
    this.notifyTimer = setTimeout(() => {
      this.notifyTimer = null;
      this.version++;
      for (const listener of this.listeners) listener();
    }, 50);
  }

  reset(
    jobId: string,
    meta?: { share: string; localDir: string; startedAt: number }
  ) {
    this.jobId = jobId;
    this.share = meta?.share ?? null;
    this.localDir = meta?.localDir ?? null;
    this.startedAt = meta?.startedAt ?? null;
    this.finishedAt = null;
    this.rows = [];
    this.logs = [];
    this.job = null;
    this.activeCount = 0;
    this.retries.clear();
    this.bump();
  }

  setJob(job: JobEvent | null) {
    this.job = job;
    if (!job || job.status !== "running") {
      this.rows = [];
      this.activeCount = 0;
    }
    this.bump();
  }

  /** Replace, rather than append, the complete active-transfer snapshot. */
  replaceActiveProgress(files: ActiveFileProgress[]) {
    this.rows = files.map((file) => ({
      key: file.path,
      status: "active" as const,
      ...file,
    }));
    this.activeCount = this.rows.length;
    this.bump();
  }

  /** Record the job finish time (idempotent). */
  finish() {
    if (this.finishedAt == null) {
      this.finishedAt = Date.now();
      this.bump();
    }
  }

  /** Track only a bounded window of failed/retrying files. */
  upsertRetry(event: RetryEvent) {
    if (event.state !== "retrying") {
      this.retries.delete(event.path);
    } else {
      if (!this.retries.has(event.path) && this.retries.size >= MAX_RETRY_ITEMS) {
        const oldest = this.retries.keys().next().value;
        if (oldest) this.retries.delete(oldest);
      }
      this.retries.set(event.path, {
        path: event.path,
        attempt: event.attempt,
        maxRetries: event.maxRetries,
        retryAt: Date.now() + event.retryIn * 1000,
      });
    }
    this.bump();
  }

  addLog(entry: LogEntry) {
    this.logs.push(entry);
    if (this.logs.length > MAX_LOGS) {
      this.logs.splice(0, this.logs.length - MAX_LOGS);
    }
    this.bump();
  }
}

export const syncStore = new SyncStore();

// Log payloads are also persisted by the backend in UTC-dated files.
export type { LogEvent };
