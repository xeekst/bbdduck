//! SSH 隧道（本地 / 远程 / 动态端口转发）的配置与状态模型。
//! serde 输出统一 camelCase，与前端 `src/lib/ssh-types.ts` 保持一致。

use serde::{Deserialize, Serialize};

// ---------------- 枚举 ----------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TunnelType {
    Local,
    Remote,
    Dynamic,
}

impl TunnelType {
    pub fn as_str(&self) -> &'static str {
        match self {
            TunnelType::Local => "local",
            TunnelType::Remote => "remote",
            TunnelType::Dynamic => "dynamic",
        }
    }
    pub fn from_str(s: &str) -> Self {
        match s {
            "remote" => TunnelType::Remote,
            "dynamic" => TunnelType::Dynamic,
            _ => TunnelType::Local,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TunnelProto {
    Tcp,
    Udp,
}

impl TunnelProto {
    pub fn as_str(&self) -> &'static str {
        match self {
            TunnelProto::Tcp => "tcp",
            TunnelProto::Udp => "udp",
        }
    }
    pub fn from_str(s: &str) -> Self {
        match s {
            "udp" => TunnelProto::Udp,
            _ => TunnelProto::Tcp,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AuthKind {
    Password,
    Key,
}

impl AuthKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            AuthKind::Password => "password",
            AuthKind::Key => "key",
        }
    }
    pub fn from_str(s: &str) -> Self {
        match s {
            "key" => AuthKind::Key,
            _ => AuthKind::Password,
        }
    }
}

// ---------------- 配置 ----------------

fn default_proto() -> TunnelProto {
    TunnelProto::Tcp
}
fn default_ssh_port() -> u16 {
    22
}
fn default_listen_host() -> String {
    "127.0.0.1".into()
}
fn default_keepalive() -> u32 {
    30
}
fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelConfig {
    /// 0 表示未保存，保存后由数据库分配
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub name: String,
    pub tunnel_type: TunnelType,
    #[serde(default = "default_proto")]
    pub proto: TunnelProto,
    #[serde(default)]
    pub ssh_host: String,
    #[serde(default = "default_ssh_port")]
    pub ssh_port: u16,
    #[serde(default)]
    pub username: String,
    pub auth: AuthKind,
    /// 密码（保存在本机 SQLite 中）
    #[serde(default)]
    pub password: Option<String>,
    /// 私钥文件路径
    #[serde(default)]
    pub key_path: Option<String>,
    /// 私钥口令（可选）
    #[serde(default)]
    pub key_passphrase: Option<String>,
    /// 监听地址（本地转发/动态：本机监听；远程转发：SSH 服务器上监听）
    #[serde(default = "default_listen_host")]
    pub listen_host: String,
    #[serde(default)]
    pub listen_port: u16,
    /// 目标主机（相对 SSH 服务器而言；动态转发不使用）
    #[serde(default)]
    pub target_host: String,
    #[serde(default)]
    pub target_port: u16,
    /// 保活间隔（秒）
    #[serde(default = "default_keepalive")]
    pub keepalive_secs: u32,
    /// 断线后自动重连
    #[serde(default = "default_true")]
    pub auto_reconnect: bool,
    /// 应用启动时自动运行
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub created_at: i64,
}

impl TunnelConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("请填写隧道名称".into());
        }
        if self.ssh_host.trim().is_empty() {
            return Err("请填写 SSH 主机".into());
        }
        if self.ssh_port == 0 {
            return Err("SSH 端口无效".into());
        }
        if self.username.trim().is_empty() {
            return Err("请填写 SSH 用户名".into());
        }
        match self.auth {
            AuthKind::Password => {
                if self.password.as_deref().unwrap_or("").is_empty() {
                    return Err("请填写 SSH 密码".into());
                }
            }
            AuthKind::Key => {
                if self.key_path.as_deref().unwrap_or("").is_empty() {
                    return Err("请选择私钥文件".into());
                }
            }
        }
        if self.listen_host.trim().is_empty() {
            return Err("请填写监听地址".into());
        }
        if self.listen_port == 0 {
            return Err("请填写监听端口".into());
        }
        if self.tunnel_type != TunnelType::Dynamic {
            if self.target_host.trim().is_empty() {
                return Err("请填写目标主机".into());
            }
            if self.target_port == 0 {
                return Err("请填写目标端口".into());
            }
        }
        Ok(())
    }

    pub fn ssh_server_display(&self) -> String {
        format!("{}@{}:{}", self.username, self.ssh_host, self.ssh_port)
    }
}

// ---------------- 运行状态 ----------------

pub const STATE_CONNECTING: &str = "connecting";
pub const STATE_RUNNING: &str = "running";
pub const STATE_STOPPING: &str = "stopping";
pub const STATE_STOPPED: &str = "stopped";
pub const STATE_ERROR: &str = "error";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelStatus {
    pub state: String,
    pub error: Option<String>,
    pub connections: u64,
    pub bytes_up: u64,
    pub bytes_down: u64,
    pub connected_at: Option<i64>,
    pub listen_addr: String,
}

impl Default for TunnelStatus {
    fn default() -> Self {
        TunnelStatus {
            state: STATE_STOPPED.into(),
            error: None,
            connections: 0,
            bytes_up: 0,
            bytes_down: 0,
            connected_at: None,
            listen_addr: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelLogEntry {
    pub level: String,
    pub message: String,
    pub time: i64,
}

/// 列表项 = 配置 + 运行状态扁平合并（id 只出现一次）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelItem {
    #[serde(flatten)]
    pub config: TunnelConfig,
    #[serde(flatten)]
    pub status: TunnelStatus,
}
