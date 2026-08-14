import type { FileProgressEvent, JobEvent, LogEvent, RetryEvent } from "./sync-types";

/** Hard caps so the UI stays fast even for hundreds of TB of files. */
export const MAX_ROWS = 50000;
export const MAX_TREE_FILES = 200000;
export const MAX_LOGS = 1000;

export interface TransferRow {
  key: string;
  path: string;
  done: number;
  total: number;
  speed: number;
  status: "active" | "done" | "error" | "deleted";
}

export interface LogEntry {
  time: number;
  level: "info" | "warn" | "error";
  message: string;
}

export interface RetryItem {
  path: string;
  attempt: number;
  maxRetries: number;
  /** Timestamp (ms) when the next attempt starts. */
  retryAt: number;
}

interface FileNode {
  name: string;
  size: number;
}

export interface DirNode {
  name: string;
  dirs: Map<string, DirNode>;
  files: Map<string, FileNode>;
  fileCount: number;
  size: number;
}

function emptyDir(name: string): DirNode {
  return { name, dirs: new Map(), files: new Map(), fileCount: 0, size: 0 };
}

class CompletedTree {
  root: DirNode = emptyDir("");
  count = 0;

  reset() {
    this.root = emptyDir("");
    this.count = 0;
  }

  addFiles(files: { path: string; size: number }[]) {
    for (const f of files) {
      if (this.count >= MAX_TREE_FILES) return;
      const parts = f.path.split("/").filter(Boolean);
      if (parts.length === 0) continue;
      let node = this.root;
      for (let i = 0; i < parts.length - 1; i++) {
        let next = node.dirs.get(parts[i]);
        if (!next) {
          next = emptyDir(parts[i]);
          node.dirs.set(parts[i], next);
        }
        node = next;
      }
      const name = parts[parts.length - 1];
      node.files.set(name, { name, size: f.size });
      node.fileCount++;
      node.size += f.size;
      this.count++;
    }
  }
}

class SyncStore {
  version = 0;
  private listeners = new Set<() => void>();

  jobId: string | null = null;
  rows: TransferRow[] = [];
  private rowMap = new Map<string, TransferRow>();
  tree = new CompletedTree();
  logs: LogEntry[] = [];
  job: JobEvent | null = null;
  activeCount = 0;
  doneCount = 0;
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
    this.version++;
    for (const l of this.listeners) l();
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
    this.rowMap.clear();
    this.tree.reset();
    this.logs = [];
    this.job = null;
    this.activeCount = 0;
    this.doneCount = 0;
    this.retries.clear();
    this.bump();
  }

  setJob(job: JobEvent | null) {
    this.job = job;
    this.bump();
  }

  /** Record the job finish time (idempotent). */
  finish() {
    if (this.finishedAt == null) {
      this.finishedAt = Date.now();
      this.bump();
    }
  }

  upsertProgress(p: FileProgressEvent) {
    const key = p.path;
    let row = this.rowMap.get(key);
    if (row) {
      const wasDone = row.status === "done";
      row.done = p.done;
      row.total = p.total;
      row.speed = p.speed;
      row.status = p.done >= p.total ? "done" : "active";
      if (!wasDone && row.status === "done") {
        this.activeCount = Math.max(0, this.activeCount - 1);
        this.doneCount++;
      }
    } else {
      if (this.rows.length >= MAX_ROWS) {
        const evicted = this.rows.shift();
        if (evicted) {
          this.rowMap.delete(evicted.key);
          if (evicted.status === "active") this.activeCount = Math.max(0, this.activeCount - 1);
          else this.doneCount = Math.max(0, this.doneCount - 1);
        }
      }
      row = {
        key,
        path: p.path,
        done: p.done,
        total: p.total,
        speed: p.speed,
        status: p.done >= p.total ? "done" : "active",
      };
      this.rows.push(row);
      this.rowMap.set(key, row);
      if (row.status === "active") this.activeCount++;
      else this.doneCount++;
    }
    this.bump();
  }

  addFilesDone(files: { path: string; size: number }[]) {
    this.tree.addFiles(files);
    for (const f of files) {
      this.retries.delete(f.path); // a retried file that succeeded leaves the queue
      const row = this.rowMap.get(f.path);
      if (row) {
        if (row.status === "active") {
          this.activeCount = Math.max(0, this.activeCount - 1);
          this.doneCount++;
        }
        row.status = "done";
        row.done = row.total || f.size;
      }
    }
    this.bump();
  }

  /** Track files that failed and are queued for retry. */
  upsertRetry(e: RetryEvent) {
    if (e.state === "failed") {
      this.retries.delete(e.path);
      let row = this.rowMap.get(e.path);
      if (!row) {
        if (this.rows.length >= MAX_ROWS) {
          const evicted = this.rows.shift();
          if (evicted) {
            this.rowMap.delete(evicted.key);
            if (evicted.status === "active") this.activeCount = Math.max(0, this.activeCount - 1);
            else if (evicted.status === "done") this.doneCount = Math.max(0, this.doneCount - 1);
          }
        }
        row = { key: e.path, path: e.path, done: 0, total: 0, speed: 0, status: "error" };
        this.rows.push(row);
        this.rowMap.set(e.path, row);
      } else if (row.status !== "done" && row.status !== "deleted") {
        if (row.status === "active") {
          this.activeCount = Math.max(0, this.activeCount - 1);
        }
        row.status = "error";
        row.speed = 0;
      }
    } else {
      this.retries.set(e.path, {
        path: e.path,
        attempt: e.attempt,
        maxRetries: e.maxRetries,
        retryAt: Date.now() + e.retryIn * 1000,
      });
    }
    this.bump();
  }

  /** Show files/dirs deleted during mirror sync as rows in the transfer list. */
  addDeleted(paths: string[]) {
    for (const p of paths) {
      if (this.rowMap.has(p)) continue;
      if (this.rows.length >= MAX_ROWS) {
        const evicted = this.rows.shift();
        if (evicted) {
          this.rowMap.delete(evicted.key);
          if (evicted.status === "active") this.activeCount = Math.max(0, this.activeCount - 1);
          else this.doneCount = Math.max(0, this.doneCount - 1);
        }
      }
      const row: TransferRow = { key: p, path: p, done: 0, total: 0, speed: 0, status: "deleted" };
      this.rows.push(row);
      this.rowMap.set(p, row);
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

// ---- Log payload from the backend (LogEvent) is { id, level, message, time } ----
export type { LogEvent };
