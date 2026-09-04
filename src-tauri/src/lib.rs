pub mod db;
pub mod file_occupancy;
pub mod net_tool;
pub mod port_occupancy;
pub mod ssh_tunnel;
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
            scanned_files: p.scanned_files,
            active_files: p.active_files,
            listing_complete: p.listing_complete,
            list_attempt: p.list_attempt,
            phase: p.phase.clone(),
            activity: p.activity.clone(),
            current_file: p.current_file.clone(),
            error: p.error.clone(),
            started_at: self.started_at,
            finished_at: *self.finished_at.lock().unwrap(),
        }
    }
}

pub struct AppState {
    pub db: Arc<Db>,
    pub server: Arc<ServerHandle>,
    pub jobs: Mutex<HashMap<String, Arc<JobHandle>>>,
    pub tunnels: Arc<ssh_tunnel::TunnelManager>,
}

// ---------------- server (Node A) ----------------

#[tauri::command]
fn server_start(
    state: State<'_, Arc<AppState>>,
    app: AppHandle,
    ip: String,
    port: u16,
    folders: Vec<String>,
    scan_workers: usize,
) -> Result<ServerStatus, String> {
    let addr = match state
        .server
        .start_with_app(app.clone(), ip, port, folders, scan_workers)
    {
        Ok(addr) => addr,
        Err(error) => {
            sync::logging::server_error(&app, format!("同步服务端启动失败：{error}"));
            return Err(error);
        }
    };
    let shares = state.server.shares();
    let status = ServerStatus {
        running: true,
        addr: Some(addr.clone()),
        shares: shares.clone(),
        connections: state.server.connections(),
    };
    let _ = app.emit(
        EVT_SERVER,
        ServerEventPayload {
            running: true,
            addr: Some(addr),
            shares,
            connections: state.server.connections(),
            message: None,
        },
    );
    sync::logging::server_info(
        &app,
        format!(
            "同步服务端已启动，监听 {}",
            status.addr.as_deref().unwrap_or("-")
        ),
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
            connections: vec![],
            message: None,
        },
    );
    sync::logging::server_info(&app, "同步服务端已停止");
    Ok(())
}

#[tauri::command]
fn server_status(state: State<'_, Arc<AppState>>) -> ServerStatus {
    ServerStatus {
        running: state.server.is_running(),
        addr: state.server.addr(),
        shares: state.server.shares(),
        connections: state.server.connections(),
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
        let (total_files, total_bytes, _skipped_paths) =
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
    state
        .jobs
        .lock()
        .unwrap()
        .insert(id.clone(), Arc::clone(&handle));

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
                scanned_files: p.scanned_files,
                active_files: p.active_files,
                listing_complete: p.listing_complete,
                list_attempt: p.list_attempt,
                phase: p.phase.clone(),
                activity: p.activity.clone(),
                current_file: p.current_file.clone(),
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
        let mut p = job.progress.lock().unwrap();
        p.phase = "stopped".into();
        p.activity = "正在停止同步…".into();
        p.current_file = None;
        p.active_files = 0;
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
            scanned_files: p.scanned_files,
            active_files: p.active_files,
            listing_complete: p.listing_complete,
            list_attempt: p.list_attempt,
            phase: p.phase.clone(),
            activity: p.activity.clone(),
            current_file: p.current_file.clone(),
            message: Some("正在停止同步…".into()),
        };
        drop(p);
        let _ = app.emit(EVT_JOB, payload);
        engine::log_info(&app, &job_id, "正在停止同步…");
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

fn completed_path(root: &std::path::Path, relative: &str) -> Result<std::path::PathBuf, String> {
    let mut path = root.to_path_buf();
    for component in std::path::Path::new(relative).components() {
        match component {
            std::path::Component::Normal(part) => path.push(part),
            std::path::Component::CurDir => {}
            _ => return Err("目录路径不合法".into()),
        }
    }
    Ok(path)
}

fn list_completed_page(
    root: String,
    relative: String,
    offset: usize,
    limit: usize,
) -> Result<CompletedPage, String> {
    let root = std::path::PathBuf::from(root);
    let dir = completed_path(&root, &relative)?;
    let page_size = limit.clamp(1, 500);
    let read_dir = std::fs::read_dir(&dir)
        .map_err(|e| format!("读取本地完成目录 {} 失败: {e}", dir.display()))?;
    let mut entries = Vec::with_capacity(page_size + 1);
    for item in read_dir.skip(offset) {
        let entry = match item {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(_) => continue,
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = if relative.is_empty() {
            name.clone()
        } else {
            format!("{}/{}", relative.trim_end_matches('/'), name)
        };
        let metadata = entry.metadata().ok();
        let modified = metadata
            .as_ref()
            .and_then(|value| value.modified().ok())
            .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|value| value.as_secs() as i64);
        entries.push(CompletedEntry {
            name,
            path,
            is_dir: file_type.is_dir(),
            size: metadata.as_ref().map(|value| value.len()).unwrap_or(0),
            modified,
        });
        if entries.len() > page_size {
            break;
        }
    }
    let has_more = entries.len() > page_size;
    entries.truncate(page_size);
    entries.sort_unstable_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(CompletedPage {
        relative,
        offset,
        has_more,
        entries,
    })
}

#[tauri::command]
async fn sync_list_completed(
    root: String,
    relative: String,
    offset: usize,
    limit: usize,
) -> Result<CompletedPage, String> {
    tauri::async_runtime::spawn_blocking(move || list_completed_page(root, relative, offset, limit))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod completed_page_tests {
    use super::*;

    #[test]
    fn pages_local_directory_without_collecting_every_entry() {
        let root = std::env::temp_dir().join(format!(
            "bbdduck-completed-page-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("folder")).unwrap();
        std::fs::write(root.join("a.txt"), b"a").unwrap();
        std::fs::write(root.join("b.txt"), b"bb").unwrap();
        std::fs::write(root.join("c.txt"), b"ccc").unwrap();

        let first =
            list_completed_page(root.to_string_lossy().into_owned(), "".into(), 0, 2).unwrap();
        assert_eq!(first.entries.len(), 2);
        assert!(first.has_more);

        let second =
            list_completed_page(root.to_string_lossy().into_owned(), "".into(), 2, 2).unwrap();
        assert_eq!(second.entries.len(), 2);
        assert!(!second.has_more);
        assert!(completed_path(&root, "../outside").is_err());

        let _ = std::fs::remove_dir_all(&root);
    }
}
// ---------------- local sqlite storage ----------------

#[tauri::command]
fn save_server_config(
    state: State<'_, Arc<AppState>>,
    name: String,
    ip: String,
    port: u16,
    folders: Vec<String>,
    scan_workers: i64,
) -> Result<i64, String> {
    state
        .db
        .save_server_config(&name, &ip, port, &folders, scan_workers)
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
fn list_recent_connections(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<RecentConnection>, String> {
    state
        .db
        .list_recent_connections()
        .map_err(|e| e.to_string())
}

// ---------------- local file occupancy ----------------

#[tauri::command]
async fn file_occupancy_scan(query: String) -> Result<file_occupancy::OccupancyScanResult, String> {
    tauri::async_runtime::spawn_blocking(move || file_occupancy::scan(query))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn file_occupancy_terminate(pid: u32, process_token: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || file_occupancy::terminate(pid, process_token))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn port_occupancy_scan(port: u16) -> Result<port_occupancy::PortOccupancyScanResult, String> {
    tauri::async_runtime::spawn_blocking(move || port_occupancy::scan(port))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn tcp_connection_stats(
    port: u16,
    source_ip: Option<String>,
    local_ip: Option<String>,
) -> Result<port_occupancy::TcpConnectionStatistics, String> {
    tauri::async_runtime::spawn_blocking(move || {
        port_occupancy::tcp_statistics(port, source_ip, local_ip)
    })
    .await
    .map_err(|error| error.to_string())?
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

// ---------------- ssh tunnels (port forwarding) ----------------

#[tauri::command]
async fn ssh_tunnel_list(
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<ssh_tunnel::model::TunnelItem>, String> {
    state.tunnels.list().await
}

#[tauri::command]
async fn ssh_tunnel_save(
    state: State<'_, Arc<AppState>>,
    config: ssh_tunnel::model::TunnelConfig,
) -> Result<ssh_tunnel::model::TunnelItem, String> {
    config.validate()?;
    // 修改配置时若隧道正在运行，先停止
    if config.id > 0 {
        state.tunnels.stop(config.id).await;
    }
    let id = state.db.save_tunnel(&config).map_err(|e| e.to_string())?;
    state
        .tunnels
        .list()
        .await?
        .into_iter()
        .find(|i| i.config.id == id)
        .ok_or_else(|| "隧道已保存，但读取失败".to_string())
}

#[tauri::command]
async fn ssh_tunnel_start(state: State<'_, Arc<AppState>>, id: i64) -> Result<(), String> {
    state.tunnels.start(id).await
}

#[tauri::command]
async fn ssh_tunnel_stop(state: State<'_, Arc<AppState>>, id: i64) -> Result<(), String> {
    state.tunnels.stop(id).await;
    Ok(())
}

#[tauri::command]
async fn ssh_tunnel_delete(state: State<'_, Arc<AppState>>, id: i64) -> Result<(), String> {
    state.tunnels.remove_runtime(id).await;
    state.db.delete_tunnel(id).map_err(|e| e.to_string())
}

#[tauri::command]
async fn ssh_tunnel_logs(
    state: State<'_, Arc<AppState>>,
    id: i64,
) -> Result<Vec<ssh_tunnel::model::TunnelLogEntry>, String> {
    Ok(state.tunnels.logs(id).await)
}

#[tauri::command]
async fn ssh_tunnel_clear_logs(state: State<'_, Arc<AppState>>, id: i64) -> Result<(), String> {
    state.tunnels.clear_logs(id).await;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            let db =
                Arc::new(Db::open(&dir).map_err(|e| -> Box<dyn std::error::Error> { e.into() })?);
            let tunnels = Arc::new(ssh_tunnel::TunnelManager::new(
                app.handle().clone(),
                db.clone(),
            ));
            let state = Arc::new(AppState {
                db,
                server: Arc::new(ServerHandle::new()),
                jobs: Mutex::new(HashMap::new()),
                tunnels,
            });
            app.manage(state);
            // 应用启动时自动运行 enabled 的隧道
            let auto = app.state::<Arc<AppState>>().tunnels.clone();
            tauri::async_runtime::spawn(async move {
                auto.auto_start().await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            server_start,
            server_stop,
            server_status,
            client_list_shares,
            sync_list_completed,
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
            file_occupancy_scan,
            file_occupancy_terminate,
            port_occupancy_scan,
            tcp_connection_stats,
            net_local_info,
            net_tcp_probe,
            net_ping,
            ssh_tunnel_list,
            ssh_tunnel_save,
            ssh_tunnel_start,
            ssh_tunnel_stop,
            ssh_tunnel_delete,
            ssh_tunnel_logs,
            ssh_tunnel_clear_logs,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
