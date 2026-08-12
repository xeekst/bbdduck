// Types for the network tools (本机网络信息 / TCP 端口检测 / ICMP Ping).
// Field names must match the Rust serde output (camelCase).

export interface IpInfo {
  addr: string;
  prefixLen: number;
  netmask: string;
}

export interface InterfaceInfo {
  name: string;
  mac: string | null;
  ips: IpInfo[];
  gateways: string[];
  dns: string[];
}

export interface RouteInfo {
  dest: string;
  gateway: string;
  interface: string;
  metric: number;
}

export interface LocalNetInfo {
  hostname: string;
  os: string;
  interfaces: InterfaceInfo[];
  routes: RouteInfo[];
}

export interface ProbeResult {
  success: boolean;
  host: string;
  resolvedIp: string | null;
  port: number;
  elapsedMs: number;
  /** connected | reset | timeout | unreachable | dnsFailed | badAddr | other */
  state: string;
  reason: string | null;
  route: RouteInfo | null;
  sourceIp: string | null;
}

export interface PingResult {
  success: boolean;
  host: string;
  resolvedIp: string | null;
  sent: number;
  received: number;
  avgMs: number | null;
  minMs: number | null;
  maxMs: number | null;
  lossPercent: number;
  reason: string | null;
}

export interface NetLogEvent {
  level: "info" | "warn" | "error";
  message: string;
  /** Unix 秒 */
  time: number;
}

export const EVT_NET_LOG = "net-log";
