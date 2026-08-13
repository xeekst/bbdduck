//! 隧道运行时管理：多个 SSH 隧道可同时存在，各自独立启停、计数与记录日志。
//! 日志与状态变化通过 Tauri 事件实时推送到前端。

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::sync::{watch, RwLock};

use crate::db::Db;
use crate::ssh_tunnel::model::*;
use crate::ssh_tunnel::runner::{self, TunnelEvents};
use crate::sync::model::now_secs;

pub const EVT_LOG: &str = "ssh-tunnel-log";
pub const EVT_STATE: &str = "ssh-tunnel-state";

const MAX_MEM_LOGS: usize = 2000;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEventPayload {
    pub id: i64,
    pub level: String,
    pub message: String,
    pub time: i64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StateEventPayload {
    pub id: i64,
    pub state: String,
    pub error: Option<String>,
    pub listen_addr: String,
    pub connected_at: Option<i64>,
}

// ---------------- 单个隧道的运行时 ----------------

pub struct TunnelRuntime {
    pub config: TunnelConfig,
    status: StdMutex<TunnelStatus>,
    logs: StdMutex<VecDeque<TunnelLogEntry>>,
    pub stop_tx: watch::Sender<bool>,
    pub stop_rx: watch::Receiver<bool>,
    /// 每个隧道自己的子任务（转发连接等），停止时统一 abort
    tasks: StdMutex<Vec<tokio::task::JoinHandle<()>>>,
    pub connections: AtomicU64,
    pub bytes_up: AtomicU64,
    pub bytes_down: AtomicU64,
}

impl TunnelRuntime {
    pub fn new(config: TunnelConfig) -> Self {
        let (stop_tx, stop_rx) = watch::channel(false);
        TunnelRuntime {
            config,
            status: StdMutex::new(TunnelStatus::default()),
            logs: StdMutex::new(VecDeque::new()),
            stop_tx,
            stop_rx,
            tasks: StdMutex::new(Vec::new()),
            connections: AtomicU64::new(0),
            bytes_up: AtomicU64::new(0),
            bytes_down: AtomicU64::new(0),
        }
    }

    pub fn is_stopped(&self) -> bool {
        *self.stop_rx.borrow()
    }

    pub fn set_state(&self, state: &str, error: Option<String>) {
        let mut st = self.status.lock().unwrap();
        st.state = state.into();
        st.error = error;
        if state == STATE_STOPPED {
            st.connections = 0;
            st.connected_at = None;
        }
    }

    pub fn set_connected_at(&self, t: Option<i64>) {
        self.status.lock().unwrap().connected_at = t;
    }

    pub fn set_listen_addr(&self, addr: &str) {
        self.status.lock().unwrap().listen_addr = addr.into();
    }

    pub fn snapshot(&self) -> TunnelStatus {
        let mut st = self.status.lock().unwrap().clone();
        st.connections = self.connections.load(Ordering::Relaxed);
        st.bytes_up = self.bytes_up.load(Ordering::Relaxed);
        st.bytes_down = self.bytes_down.load(Ordering::Relaxed);
        st
    }

    /// 在当前 tokio 运行时上派生一个受隧道管理的子任务
    pub fn spawn_task<F>(&self, fut: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let handle = tokio::spawn(fut);
        self.tasks.lock().unwrap().push(handle);
    }

    /// 中止全部子任务并等待它们退出
    pub async fn abort_tasks(&self) {
        let handles = std::mem::take(&mut *self.tasks.lock().unwrap());
        for h in &handles {
            h.abort();
        }
        for h in handles {
            let _ = h.await;
        }
    }
}

// ---------------- 管理器 ----------------

impl TunnelEvents for TunnelManager {
    fn log(&self, rt: &Arc<TunnelRuntime>, level: &str, message: String) {
        TunnelManager::log(self, rt, level, message);
    }

    fn emit_state(&self, rt: &Arc<TunnelRuntime>) {
        TunnelManager::emit_state(self, rt);
    }
}

pub struct TunnelManager {
    app: AppHandle,
    db: Arc<Db>,
    runtimes: RwLock<HashMap<i64, Arc<TunnelRuntime>>>,
}

impl TunnelManager {
    pub fn new(app: AppHandle, db: Arc<Db>) -> Self {
        TunnelManager {
            app,
            db,
            runtimes: RwLock::new(HashMap::new()),
        }
    }

    fn load_config(&self, id: i64) -> Result<TunnelConfig, String> {
        self.db
            .list_tunnels()
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|c| c.id == id)
            .ok_or_else(|| "隧道不存在".to_string())
    }

    /// 启动一个隧道（异步：立即返回，实际连接在后台任务中进行）
    pub async fn start(self: &Arc<Self>, id: i64) -> Result<(), String> {
        let config = self.load_config(id)?;
        config.validate()?;
        {
            let guard = self.runtimes.read().await;
            if let Some(rt) = guard.get(&id) {
                let st = rt.snapshot();
                if st.state == STATE_RUNNING
                    || st.state == STATE_CONNECTING
                    || st.state == STATE_STOPPING
                {
                    return Err("该隧道已在运行".into());
                }
            }
        }
        let rt = Arc::new(TunnelRuntime::new(config));
        self.runtimes.write().await.insert(id, rt.clone());
        let mgr: Arc<dyn TunnelEvents> = self.clone();
        tauri::async_runtime::spawn(runner::run_tunnel(mgr, rt));
        Ok(())
    }

    /// 停止一个隧道（异步发起，后台任务随后退出并推送 stopped 状态）
    pub async fn stop(&self, id: i64) {
        let rt = {
            let guard = self.runtimes.read().await;
            guard.get(&id).cloned()
        };
        if let Some(rt) = rt {
            let st = rt.snapshot();
            if st.state == STATE_STOPPED {
                return;
            }
            rt.set_state(STATE_STOPPING, None);
            self.emit_state(&rt);
            let _ = rt.stop_tx.send(true);
            rt.abort_tasks().await;
        }
    }

    /// 从运行时表中移除（删除隧道时调用）
    pub async fn remove_runtime(&self, id: i64) {
        self.stop(id).await;
        self.runtimes.write().await.remove(&id);
    }

    /// 应用启动时自动运行 enabled 的隧道
    pub async fn auto_start(self: &Arc<Self>) {
        let configs = match self.db.list_tunnels() {
            Ok(c) => c,
            Err(_) => return,
        };
        for c in configs.into_iter().filter(|c| c.enabled) {
            let id = c.id;
            if let Err(e) = self.start(id).await {
                let _ = self.db.append_tunnel_log(
                    id,
                    "error",
                    &format!("开机自启失败: {e}"),
                );
            }
        }
    }

    /// 配置列表 + 各自运行状态
    pub async fn list(&self) -> Result<Vec<TunnelItem>, String> {
        let configs = self.db.list_tunnels().map_err(|e| e.to_string())?;
        let guard = self.runtimes.read().await;
        Ok(configs
            .into_iter()
            .map(|config| {
                let status = guard
                    .get(&config.id)
                    .map(|rt| rt.snapshot())
                    .unwrap_or_else(|| {
                        let mut s = TunnelStatus::default();
                        // 尚未启动过的隧道，监听地址展示为配置值
                        s.listen_addr = if config.tunnel_type == TunnelType::Remote {
                            format!("{}:{}", config.ssh_host, config.listen_port)
                        } else {
                            format!("{}:{}", config.listen_host, config.listen_port)
                        };
                        s
                    });
                TunnelItem { config, status }
            })
            .collect())
    }

    /// 日志：运行中取内存缓冲（含最新），否则读数据库
    pub async fn logs(&self, id: i64) -> Vec<TunnelLogEntry> {
        {
            let guard = self.runtimes.read().await;
            if let Some(rt) = guard.get(&id) {
                return rt.logs.lock().unwrap().iter().cloned().collect();
            }
        }
        self.db.list_tunnel_logs(id, 2000).unwrap_or_default()
    }

    pub async fn clear_logs(&self, id: i64) {
        {
            let guard = self.runtimes.read().await;
            if let Some(rt) = guard.get(&id) {
                rt.logs.lock().unwrap().clear();
            }
        }
        let _ = self.db.delete_tunnel_logs(id);
    }

    // ---------------- 日志与事件 ----------------

    pub fn log(&self, rt: &Arc<TunnelRuntime>, level: &str, message: impl Into<String>) {
        let entry = TunnelLogEntry {
            level: level.into(),
            message: message.into(),
            time: now_secs(),
        };
        {
            let mut logs = rt.logs.lock().unwrap();
            logs.push_back(entry.clone());
            while logs.len() > MAX_MEM_LOGS {
                logs.pop_front();
            }
        }
        let _ = self
            .db
            .append_tunnel_log(rt.config.id, &entry.level, &entry.message);
        let _ = self.app.emit(
            EVT_LOG,
            LogEventPayload {
                id: rt.config.id,
                level: entry.level,
                message: entry.message,
                time: entry.time,
            },
        );
    }

    pub fn emit_state(&self, rt: &Arc<TunnelRuntime>) {
        let s = rt.snapshot();
        let _ = self.app.emit(
            EVT_STATE,
            StateEventPayload {
                id: rt.config.id,
                state: s.state,
                error: s.error,
                listen_addr: s.listen_addr,
                connected_at: s.connected_at,
            },
        );
    }
}
