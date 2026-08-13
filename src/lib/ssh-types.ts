// SSH 端口转发隧道类型（与 Rust `src-tauri/src/ssh_tunnel/model.rs` serde 输出对齐）
// 注意：Rust 端使用 `#[serde(flatten)]`，TunnelItem 是配置 + 状态字段的扁平合并。

export type TunnelType = "local" | "remote" | "dynamic";
export type TunnelProto = "tcp" | "udp";
export type AuthKind = "password" | "key";
export type TunnelState = "running" | "stopped" | "connecting" | "stopping" | "error";

export interface TunnelConfig {
  /** 0 = 未保存 */
  id: number;
  name: string;
  tunnelType: TunnelType;
  proto: TunnelProto;
  sshHost: string;
  sshPort: number;
  username: string;
  auth: AuthKind;
  password: string | null;
  keyPath: string | null;
  keyPassphrase: string | null;
  listenHost: string;
  listenPort: number;
  targetHost: string;
  targetPort: number;
  keepaliveSecs: number;
  autoReconnect: boolean;
  /** 应用启动时自动运行 */
  enabled: boolean;
  createdAt: number;
}

export interface TunnelStatus {
  state: TunnelState;
  error: string | null;
  connections: number;
  bytesUp: number;
  bytesDown: number;
  connectedAt: number | null;
  listenAddr: string;
}

/** 列表项 = 配置 + 状态扁平合并（id 只出现一次） */
export type TunnelItem = TunnelConfig & Omit<TunnelStatus, "id">;

export interface TunnelLogEntry {
  level: "info" | "warn" | "error";
  message: string;
  /** Unix 秒 */
  time: number;
}

export interface TunnelLogEvent extends TunnelLogEntry {
  id: number;
}

export interface TunnelStateEvent {
  id: number;
  state: TunnelState;
  error: string | null;
  listenAddr: string;
  connectedAt: number | null;
}

export const EVT_SSH_TUNNEL_LOG = "ssh-tunnel-log";
export const EVT_SSH_TUNNEL_STATE = "ssh-tunnel-state";

export const TUNNEL_TYPE_LABEL: Record<TunnelType, string> = {
  local: "本地转发",
  remote: "远程转发",
  dynamic: "动态转发",
};

export const TUNNEL_STATE_LABEL: Record<TunnelState, string> = {
  running: "运行中",
  stopped: "已停止",
  connecting: "连接中",
  stopping: "停止中",
  error: "异常",
};
