//! SQLite storage for all local data: saved server configs, recent
//! connections and sync job history. Uses WAL for durability + concurrency.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection};

use crate::sync::model::{now_secs, RecentConnection, ServerConfig, SyncOptions};

pub struct Db {
    conn: Mutex<Connection>,
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS server_configs (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  name TEXT NOT NULL,
  ip TEXT NOT NULL,
  port INTEGER NOT NULL,
  folder TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS recent_connections (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  ip TEXT NOT NULL,
  port INTEGER NOT NULL,
  share TEXT NOT NULL,
  local_dir TEXT NOT NULL DEFAULT '',
  last_used INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS sync_jobs (
  id TEXT PRIMARY KEY,
  status TEXT NOT NULL,
  remote TEXT NOT NULL,
  share TEXT NOT NULL,
  local_dir TEXT NOT NULL,
  threads INTEGER NOT NULL,
  incremental INTEGER NOT NULL,
  total_files INTEGER NOT NULL DEFAULT 0,
  done_files INTEGER NOT NULL DEFAULT 0,
  failed_files INTEGER NOT NULL DEFAULT 0,
  skipped_files INTEGER NOT NULL DEFAULT 0,
  total_bytes INTEGER NOT NULL DEFAULT 0,
  done_bytes INTEGER NOT NULL DEFAULT 0,
  error TEXT,
  started_at INTEGER NOT NULL,
  finished_at INTEGER
);
"#;

impl Db {
    pub fn open(dir: &Path) -> rusqlite::Result<Self> {
        let _ = std::fs::create_dir_all(dir);
        let conn = Connection::open(dir.join("bbdduck.db"))?;
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        let _ = conn.pragma_update(None, "synchronous", "NORMAL");
        conn.execute_batch(SCHEMA)?;
        // migration for databases created before the local_dir column existed
        let has_local_dir = conn
            .prepare("SELECT 1 FROM pragma_table_info('recent_connections') WHERE name = 'local_dir'")?
            .exists([])?;
        if !has_local_dir {
            conn.execute_batch(
                "ALTER TABLE recent_connections ADD COLUMN local_dir TEXT NOT NULL DEFAULT ''",
            )?;
        }
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    // ---------- server configs ----------

    pub fn save_server_config(
        &self,
        name: &str,
        ip: &str,
        port: u16,
        folders: &[String],
    ) -> rusqlite::Result<i64> {
        let folders_json = serde_json::to_string(folders).unwrap_or_else(|_| "[]".into());
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO server_configs (name, ip, port, folder, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![name, ip, port, folders_json, now_secs()],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn list_server_configs(&self) -> rusqlite::Result<Vec<ServerConfig>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, ip, port, folder, created_at FROM server_configs ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            let folders_json: String = r.get(4)?;
            Ok(ServerConfig {
                id: r.get(0)?,
                name: r.get(1)?,
                ip: r.get(2)?,
                port: r.get(3)?,
                folders: serde_json::from_str(&folders_json).unwrap_or_default(),
                created_at: r.get(5)?,
            })
        })?;
        rows.collect()
    }

    pub fn delete_server_config(&self, id: i64) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM server_configs WHERE id = ?1", params![id])?;
        Ok(())
    }

    // ---------- recent connections ----------

    pub fn save_recent_connection(
        &self,
        ip: &str,
        port: u16,
        share: &str,
        local_dir: &str,
    ) -> rusqlite::Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE recent_connections SET last_used = ?1, share = ?2, local_dir = ?3 WHERE ip = ?4 AND port = ?5",
            params![now_secs(), share, local_dir, ip, port],
        )?;
        if conn.changes() == 0 {
            conn.execute(
                "INSERT INTO recent_connections (ip, port, share, local_dir, last_used) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![ip, port, share, local_dir, now_secs()],
            )?;
            return Ok(conn.last_insert_rowid());
        }
        let id = conn.query_row(
            "SELECT id FROM recent_connections WHERE ip = ?1 AND port = ?2",
            params![ip, port],
            |r| r.get(0),
        )?;
        Ok(id)
    }

    pub fn list_recent_connections(&self) -> rusqlite::Result<Vec<RecentConnection>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, ip, port, share, local_dir, last_used FROM recent_connections ORDER BY last_used DESC LIMIT 50",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(RecentConnection {
                id: r.get(0)?,
                ip: r.get(1)?,
                port: r.get(2)?,
                share: r.get(3)?,
                local_dir: r.get(4)?,
                last_used: r.get(5)?,
            })
        })?;
        rows.collect()
    }

    // ---------- sync jobs ----------

    pub fn insert_job_start(&self, id: &str, opts: &SyncOptions, started_at: i64) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO sync_jobs (id, status, remote, share, local_dir, threads, incremental, started_at)
             VALUES (?1, 'running', ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id,
                format!("{}:{}", opts.remote_ip, opts.remote_port),
                opts.share,
                opts.local_dir,
                opts.threads as i64,
                opts.incremental as i64,
                started_at
            ],
        )?;
        Ok(())
    }

    pub fn finish_job(
        &self,
        id: &str,
        status: &str,
        total_files: u64,
        done_files: u64,
        failed_files: u64,
        skipped_files: u64,
        total_bytes: u64,
        done_bytes: u64,
        error: Option<&str>,
    ) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE sync_jobs SET status = ?1, total_files = ?2, done_files = ?3,
             failed_files = ?4, skipped_files = ?5, total_bytes = ?6, done_bytes = ?7,
             error = ?8, finished_at = ?9 WHERE id = ?10",
            params![
                status,
                total_files as i64,
                done_files as i64,
                failed_files as i64,
                skipped_files as i64,
                total_bytes as i64,
                done_bytes as i64,
                error,
                now_secs(),
                id
            ],
        )?;
        Ok(())
    }

    pub fn list_job_history(&self, limit: usize) -> rusqlite::Result<Vec<crate::sync::model::JobSnapshot>> {
        use crate::sync::model::{JobSnapshot, JobStatus};
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, status, remote, share, local_dir, threads, incremental,
                    total_files, done_files, failed_files, skipped_files,
                    total_bytes, done_bytes, error, started_at, finished_at
             FROM sync_jobs ORDER BY started_at DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], |r| {
            let status_str: String = r.get(1)?;
            Ok(JobSnapshot {
                id: r.get(0)?,
                status: match status_str.as_str() {
                    "running" => JobStatus::Running,
                    "finished" => JobStatus::Finished,
                    "stopped" => JobStatus::Stopped,
                    _ => JobStatus::Error,
                },
                remote: r.get(2)?,
                share: r.get(3)?,
                local_dir: r.get(4)?,
                threads: r.get(5)?,
                incremental: r.get::<_, i64>(6)? != 0,
                total_files: r.get::<_, i64>(7)? as u64,
                done_files: r.get::<_, i64>(8)? as u64,
                failed_files: r.get::<_, i64>(9)? as u64,
                skipped_files: r.get::<_, i64>(10)? as u64,
                total_bytes: r.get::<_, i64>(11)? as u64,
                done_bytes: r.get::<_, i64>(12)? as u64,
                speed: 0,
                error: r.get(13)?,
                started_at: r.get(14)?,
                finished_at: r.get(15)?,
            })
        })?;
        rows.collect()
    }
}
