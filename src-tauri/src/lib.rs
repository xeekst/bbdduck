pub mod db;
pub mod net_tool;
pub mod sync;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use db::Db;
use sync::engine::{self, JobProgress};
use sync::model::*;
use sync::server::ServerHandle;

use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

pub struct JobHandle {
    pub id: String,
    pub opts: SyncOptions,
    pub progress: Arc<Mutex<JobProgress>>,
    pub stop: Arc<AtomicBool>,
    pub started_at: i64,
    pub finished_at: Mutex<Option<i64>>,
    thread: Mutex<Option<thread::JoinHandle<()>>>,
}

impl JobHandle {
    fn snapshot(&self) -> JobSnapshot {
        let p = self.progress.lock().unwrap();
        JobSnapshot {
            id: self.id.clone(),
            status: if self.stop.load(Ordering::Relaxed) {
                JobStatus::Stopped
            } else {
                JobStatus::Running
            },
            remote: format!("{}:{}", self.opts.remote_ip, self.opts.remote_port),
            share: self.opts.share.clone(),
            local_dir: self.opts.local_dir.clone(),
            threads: self.opts.threads,
            incremental: self.opts.incremental,
            total_files: p.total_files,
            done_files: p.done_files,
            failed_files: p.failed_files,
            skipped_files: p.skipped_files,
            total_bytes: p.total_bytes,
            done_bytes: p.done_bytes,
            speed: p.speed,
            error: p.error.clone(),
            started_at: self.started_at,
            finished_at: *self.finished_at.lock().unwrap(),
        }
    }
}

pub struct AppState {
    pub db: Db,
    pub server: Arc<ServerHandle>,
    pub jobs: Mutex<HashMap<String, Arc<JobHandle>>>,
}

// ---------------- server (Node A) ----------------

#[tauri::command]
fn server_start(
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
    ip: String,
    port: u16,
    folders: Vec<String>,
) -> Result<ServerStatus, String> {
    let addr = state.server.start(ip, port, folders)?;
    let shares = state.server.shares();
    let status = ServerStatus {
        running: true,
        addr: Some(addr.clone()),
        shares: shares.clone(),
    };
    let _ = app.emit(
        EVT_SERVER,
        ServerEventPayload {
            running: true,
            addr: Some(addr),
            shares,
            message: None,
        },
    );
    Ok(status)
}

#[tauri::command]
fn server_stop(state: State<'_, Arc<AppState>>, app: AppHandle) -> Result<(), String> {
    state.server.stop();
    let _ = app.emit(
        EVT_SERVER,
        ServerEventPayload {
            running: false,
            addr: None,
            shares: vec![],
            message: None,
        },
    );
    Ok(())
}

#[tauri::command]
fn server_status(state: State<'_, Arc<AppState>>) -> ServerStatus {
    ServerStatus {
        running: state.server.is_running(),
        addr: state.server.addr(),
        shares: state.server.shares(),
    }
}

// ---------------- client (Node B) ----------------

#[tauri::command]
async fn client_list_shares(ip: String, port: u16) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || sync::client::list_shares(&ip, port))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn client_remote_info(ip: String, port: u16, share: String) -> Result<RemoteInfo, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (total_files, total_bytes) =
            sync::client::list_remote_files(&ip, port, &share, |_| true)?;
        Ok::<RemoteInfo, String>(RemoteInfo {
            total_files,
            total_bytes,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

// ---------------- sync jobs ----------------

#[tauri::command]
fn sync_start(
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
    opts: SyncOptions,
) -> Result<String, String> {
    let id = Uuid::new_v4().to_string();
    let stop = Arc::new(AtomicBool::new(false));
    let progress = Arc::new(Mutex::new(JobProgress::default()));
    let started_at = now_secs();
    let _ = state.db.insert_job_start(&id, &opts, started_at);

    let handle = Arc::new(JobHandle {
        id: id.clone(),
        opts: opts.clone(),
        progress: Arc::clone(&progress),
        stop: Arc::clone(&stop),
        started_at,
        finished_at: Mutex::new(None),
        thread: Mutex::new(None),
    });
    state.jobs.lock().unwrap().insert(id.clone(), Arc::clone(&handle));

    let state2 = state.inner().clone();
    let app2 = app.clone();
    let handle2 = Arc::clone(&handle);
    let id2 = id.clone();
    let progress2 = Arc::clone(&progress);
    let job_thread = thread::spawn(move || {
        let summary = engine::run_job(&app2, &id2, opts, stop, progress2);

        // final job event
        let final_payload = {
            let p = progress.lock().unwrap();
            JobEventPayload {
                id: id2.clone(),
                status: summary.status,
                total_files: p.total_files,
                done_files: p.done_files,
                failed_files: p.failed_files,
                skipped_files: p.skipped_files,
                total_bytes: p.total_bytes,
                done_bytes: p.done_bytes,
                speed: 0,
                message: summary.error.clone().or_else(|| summary.message.clone()),
            }
        };
        let _ = app2.emit(EVT_JOB, final_payload);

        // persist to sqlite
        {
            let p = progress.lock().unwrap();
            let _ = state2.db.finish_job(
                &id2,
                summary.status.as_str(),
                p.total_files,
                p.done_files,
                p.failed_files,
                p.skipped_files,
                p.total_bytes,
                p.done_bytes,
                summary.error.as_deref(),
            );
        }
        *handle2.finished_at.lock().unwrap() = Some(now_secs());
        state2.jobs.lock().unwrap().remove(&id2);

        match summary.status {
            JobStatus::Finished => engine::log_info(&app2, &id2, "同步完成"),
            JobStatus::Stopped => engine::log_info(&app2, &id2, "同步已停止"),
            JobStatus::Error => engine::log_error(
                &app2,
                &id2,
                format!("同步失败: {}", summary.error.unwrap_or_default()),
            ),
            JobStatus::Running => {}
        }
    });
    *handle.thread.lock().unwrap() = Some(job_thread);
    Ok(id)
}

#[tauri::command]
async fn sync_stop(app: AppHandle, job_id: String) -> Result<(), String> {
    // Run on the async pool (not the main thread) so it always responds quickly.
    let state = app.state::<Arc<AppState>>();
    if let Some(job) = state.jobs.lock().unwrap().get(&job_id).cloned() {
        job.stop.store(true, Ordering::Relaxed);

        // Report "stopped" immediately so the UI resets right away, even while
        // the engine is still unwinding its threads in the background.
        let p = job.progress.lock().unwrap();
        let payload = JobEventPayload {
            id: job_id.clone(),
            status: JobStatus::Stopped,
            total_files: p.total_files,
            done_files: p.done_files,
            failed_files: p.failed_files,
            skipped_files: p.skipped_files,
            total_bytes: p.total_bytes,
            done_bytes: p.done_bytes,
            speed: 0,
            message: Some("正在停止同步…".into()),
        };
        drop(p);
        let _ = app.emit(EVT_JOB, payload);
        let _ = app.emit(
            EVT_LOG,
            LogPayload {
                id: job_id.clone(),
                level: "info".into(),
                message: "正在停止同步…".into(),
                time: now_secs(),
            },
        );
    }
    Ok(())
}

#[tauri::command]
fn sync_active_jobs(state: State<'_, Arc<AppState>>) -> Vec<JobSnapshot> {
    state
        .jobs
        .lock()
        .unwrap()
        .values()
        .map(|j| j.snapshot())
        .collect()
}

#[tauri::command]
fn sync_history(state: State<'_, Arc<AppState>>, limit: usize) -> Result<Vec<JobSnapshot>, String> {
    state
        .db
        .list_job_history(limit.max(1))
        .map_err(|e| e.to_string())
}

// ---------------- local sqlite storage ----------------

#[tauri::command]
fn save_server_config(
    state: State<'_, Arc<AppState>>,
    name: String,
    ip: String,
    port: u16,
    folders: Vec<String>,
) -> Result<i64, String> {
    state
        .db
        .save_server_config(&name, &ip, port, &folders)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn list_server_configs(state: State<'_, Arc<AppState>>) -> Result<Vec<ServerConfig>, String> {
    state.db.list_server_configs().map_err(|e| e.to_string())
}

#[tauri::command]
fn delete_server_config(state: State<'_, Arc<AppState>>, id: i64) -> Result<(), String> {
    state.db.delete_server_config(id).map_err(|e| e.to_string())
}

#[tauri::command]
fn save_recent_connection(
    state: State<'_, Arc<AppState>>,
    ip: String,
    port: u16,
    share: String,
    local_dir: String,
) -> Result<i64, String> {
    state
        .db
        .save_recent_connection(&ip, port, &share, &local_dir)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn list_recent_connections(state: State<'_, Arc<AppState>>) -> Result<Vec<RecentConnection>, String> {
    state
        .db
        .list_recent_connections()
        .map_err(|e| e.to_string())
}

// ---------------- network tools ----------------

#[tauri::command]
fn net_local_info() -> Result<net_tool::LocalNetInfo, String> {
    net_tool::net_local_info()
}

#[tauri::command]
async fn net_tcp_probe(
    app: AppHandle,
    host: String,
    port: u16,
    timeout_ms: u64,
) -> Result<net_tool::ProbeResult, String> {
    net_tool::net_tcp_probe(app, host, port, timeout_ms).await
}

#[tauri::command]
async fn net_ping(
    app: AppHandle,
    host: String,
    count: u32,
    timeout_ms: u64,
) -> Result<net_tool::PingResult, String> {
    net_tool::net_ping(app, host, count, timeout_ms).await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            let db = Db::open(&dir).map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
            let state = Arc::new(AppState {
                db,
                server: Arc::new(ServerHandle::new()),
                jobs: Mutex::new(HashMap::new()),
            });
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            server_start,
            server_stop,
            server_status,
            client_list_shares,
            client_remote_info,
            sync_start,
            sync_stop,
            sync_active_jobs,
            sync_history,
            save_server_config,
            list_server_configs,
            delete_server_config,
            save_recent_connection,
            list_recent_connections,
            net_local_info,
            net_tcp_probe,
            net_ping,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
