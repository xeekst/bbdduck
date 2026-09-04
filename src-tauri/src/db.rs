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
  scan_workers INTEGER NOT NULL DEFAULT 0,
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
CREATE TABLE IF NOT EXISTS ssh_tunnels (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  name TEXT NOT NULL,
  tunnel_type TEXT NOT NULL,
  proto TEXT NOT NULL DEFAULT 'tcp',
  ssh_host TEXT NOT NULL,
  ssh_port INTEGER NOT NULL DEFAULT 22,
  username TEXT NOT NULL,
  auth TEXT NOT NULL DEFAULT 'password',
  password TEXT,
  key_path TEXT,
  key_passphrase TEXT,
  listen_host TEXT NOT NULL,
  listen_port INTEGER NOT NULL,
  target_host TEXT NOT NULL DEFAULT '',
  target_port INTEGER NOT NULL DEFAULT 0,
  keepalive_secs INTEGER NOT NULL DEFAULT 30,
  auto_reconnect INTEGER NOT NULL DEFAULT 1,
  enabled INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS ssh_tunnel_logs (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  tunnel_id INTEGER NOT NULL,
  level TEXT NOT NULL,
  message TEXT NOT NULL,
  time INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_ssh_tunnel_logs ON ssh_tunnel_logs (tunnel_id, id);
"#;

impl Db {
    pub fn open(dir: &Path) -> rusqlite::Result<Self> {
        let _ = std::fs::create_dir_all(dir);
        let db_path = dir.join("bbq-duck.db");
        let legacy_db_path = dir.join("bbdduck.db");
        let db_path = if !db_path.exists() && legacy_db_path.exists() {
            // Checkpoint a legacy WAL before renaming the database. If the
            // rename fails, keep using the legacy path instead of losing data.
            if let Ok(legacy_conn) = Connection::open(&legacy_db_path) {
                let _ = legacy_conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
            }
            if std::fs::rename(&legacy_db_path, &db_path).is_ok() {
                db_path
            } else {
                legacy_db_path
            }
        } else {
            db_path
        };
        let conn = Connection::open(db_path)?;
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        let _ = conn.pragma_update(None, "synchronous", "NORMAL");
        conn.execute_batch(SCHEMA)?;
        // migration for databases created before the local_dir column existed
        let has_local_dir = conn
            .prepare(
                "SELECT 1 FROM pragma_table_info('recent_connections') WHERE name = 'local_dir'",
            )?
            .exists([])?;
        if !has_local_dir {
            conn.execute_batch(
                "ALTER TABLE recent_connections ADD COLUMN local_dir TEXT NOT NULL DEFAULT ''",
            )?;
        }
        // migration for databases created before the scan_workers column existed
        let has_scan_workers = conn
            .prepare(
                "SELECT 1 FROM pragma_table_info('server_configs') WHERE name = 'scan_workers'",
            )?
            .exists([])?;
        if !has_scan_workers {
            conn.execute_batch(
                "ALTER TABLE server_configs ADD COLUMN scan_workers INTEGER NOT NULL DEFAULT 0",
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
        scan_workers: i64,
    ) -> rusqlite::Result<i64> {
        let folders_json = serde_json::to_string(folders).unwrap_or_else(|_| "[]".into());
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO server_configs (name, ip, port, folder, scan_workers, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![name, ip, port, folders_json, scan_workers, now_secs()],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn list_server_configs(&self) -> rusqlite::Result<Vec<ServerConfig>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, ip, port, folder, scan_workers, created_at FROM server_configs ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            let folders_json: String = r.get(4)?;
            Ok(ServerConfig {
                id: r.get(0)?,
                name: r.get(1)?,
                ip: r.get(2)?,
                port: r.get(3)?,
                folders: serde_json::from_str(&folders_json).unwrap_or_default(),
                scan_workers: r.get(5)?,
                created_at: r.get(6)?,
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

    pub fn insert_job_start(
        &self,
        id: &str,
        opts: &SyncOptions,
        started_at: i64,
    ) -> rusqlite::Result<()> {
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

    pub fn list_job_history(
        &self,
        limit: usize,
    ) -> rusqlite::Result<Vec<crate::sync::model::JobSnapshot>> {
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
                scanned_files: 0,
                active_files: 0,
                listing_complete: true,
                list_attempt: 1,
                phase: "finished".into(),
                activity: "历史同步任务".into(),
                current_file: None,
                error: r.get(13)?,
                started_at: r.get(14)?,
                finished_at: r.get(15)?,
            })
        })?;
        rows.collect()
    }

    // ---------- ssh tunnels ----------

    pub fn save_tunnel(&self, c: &crate::ssh_tunnel::model::TunnelConfig) -> rusqlite::Result<i64> {
        let conn = self.conn.lock().unwrap();
        if c.id > 0 {
            conn.execute(
                "UPDATE ssh_tunnels SET name = ?1, tunnel_type = ?2, proto = ?3, ssh_host = ?4,
                 ssh_port = ?5, username = ?6, auth = ?7, password = ?8, key_path = ?9,
                 key_passphrase = ?10, listen_host = ?11, listen_port = ?12, target_host = ?13,
                 target_port = ?14, keepalive_secs = ?15, auto_reconnect = ?16, enabled = ?17
                 WHERE id = ?18",
                params![
                    c.name,
                    c.tunnel_type.as_str(),
                    c.proto.as_str(),
                    c.ssh_host,
                    c.ssh_port,
                    c.username,
                    c.auth.as_str(),
                    c.password,
                    c.key_path,
                    c.key_passphrase,
                    c.listen_host,
                    c.listen_port,
                    c.target_host,
                    c.target_port,
                    c.keepalive_secs,
                    c.auto_reconnect as i64,
                    c.enabled as i64,
                    c.id
                ],
            )?;
            return Ok(c.id);
        }
        conn.execute(
            "INSERT INTO ssh_tunnels (name, tunnel_type, proto, ssh_host, ssh_port, username, auth,
             password, key_path, key_passphrase, listen_host, listen_port, target_host, target_port,
             keepalive_secs, auto_reconnect, enabled, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
            params![
                c.name,
                c.tunnel_type.as_str(),
                c.proto.as_str(),
                c.ssh_host,
                c.ssh_port,
                c.username,
                c.auth.as_str(),
                c.password,
                c.key_path,
                c.key_passphrase,
                c.listen_host,
                c.listen_port,
                c.target_host,
                c.target_port,
                c.keepalive_secs,
                c.auto_reconnect as i64,
                c.enabled as i64,
                crate::sync::model::now_secs()
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn list_tunnels(&self) -> rusqlite::Result<Vec<crate::ssh_tunnel::model::TunnelConfig>> {
        use crate::ssh_tunnel::model::{AuthKind, TunnelConfig, TunnelProto, TunnelType};
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, tunnel_type, proto, ssh_host, ssh_port, username, auth, password,
                    key_path, key_passphrase, listen_host, listen_port, target_host, target_port,
                    keepalive_secs, auto_reconnect, enabled, created_at
             FROM ssh_tunnels ORDER BY created_at DESC, id DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(TunnelConfig {
                id: r.get(0)?,
                name: r.get(1)?,
                tunnel_type: TunnelType::from_str(&r.get::<_, String>(2)?),
                proto: TunnelProto::from_str(&r.get::<_, String>(3)?),
                ssh_host: r.get(4)?,
                ssh_port: r.get(5)?,
                username: r.get(6)?,
                auth: AuthKind::from_str(&r.get::<_, String>(7)?),
                password: r.get(8)?,
                key_path: r.get(9)?,
                key_passphrase: r.get(10)?,
                listen_host: r.get(11)?,
                listen_port: r.get(12)?,
                target_host: r.get(13)?,
                target_port: r.get(14)?,
                keepalive_secs: r.get(15)?,
                auto_reconnect: r.get::<_, i64>(16)? != 0,
                enabled: r.get::<_, i64>(17)? != 0,
                created_at: r.get(18)?,
            })
        })?;
        rows.collect()
    }

    pub fn delete_tunnel(&self, id: i64) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM ssh_tunnels WHERE id = ?1", params![id])?;
        conn.execute(
            "DELETE FROM ssh_tunnel_logs WHERE tunnel_id = ?1",
            params![id],
        )?;
        Ok(())
    }

    pub fn append_tunnel_log(
        &self,
        tunnel_id: i64,
        level: &str,
        message: &str,
    ) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO ssh_tunnel_logs (tunnel_id, level, message, time) VALUES (?1, ?2, ?3, ?4)",
            params![tunnel_id, level, message, crate::sync::model::now_secs()],
        )?;
        // keep only the latest 2000 entries per tunnel
        let _ = conn.execute(
            "DELETE FROM ssh_tunnel_logs WHERE tunnel_id = ?1 AND id NOT IN
             (SELECT id FROM ssh_tunnel_logs WHERE tunnel_id = ?1 ORDER BY id DESC LIMIT 2000)",
            params![tunnel_id],
        );
        Ok(())
    }

    pub fn list_tunnel_logs(
        &self,
        tunnel_id: i64,
        limit: usize,
    ) -> rusqlite::Result<Vec<crate::ssh_tunnel::model::TunnelLogEntry>> {
        use crate::ssh_tunnel::model::TunnelLogEntry;
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT level, message, time FROM ssh_tunnel_logs WHERE tunnel_id = ?1
             ORDER BY id DESC LIMIT ?2",
        )?;
        let mut rows: Vec<TunnelLogEntry> = stmt
            .query_map(params![tunnel_id, limit as i64], |r| {
                Ok(TunnelLogEntry {
                    level: r.get(0)?,
                    message: r.get(1)?,
                    time: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.reverse();
        Ok(rows)
    }

    pub fn delete_tunnel_logs(&self, tunnel_id: i64) -> rusqlite::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM ssh_tunnel_logs WHERE tunnel_id = ?1",
            params![tunnel_id],
        )?;
        Ok(())
    }
}
