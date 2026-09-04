export interface TcpStateCount {
  state: string;
  count: number;
}

export interface TcpConnectionDetail {
  addressFamily: "IPv4" | "IPv6";
  localIp: string;
  localPort: number;
  remoteIp: string | null;
  remotePort: number | null;
  state: string;
  pid: number;
  processName: string;
  processPath: string | null;
  processStartedAt: number | null;
  bytesSent: number | null;
  bytesReceived: number | null;
}

export interface TcpConnectionStatistics {
  port: number;
  sourceIp: string | null;
  localIp: string | null;
  totalConnections: number;
  listenerCount: number;
  processCount: number;
  totalBytesSent: number;
  totalBytesReceived: number;
  trafficAvailableConnections: number;
  trafficUnavailableConnections: number;
  trafficNewlyEnabledConnections: number;
  trafficPermissionDenied: boolean;
  stateCounts: TcpStateCount[];
  connections: TcpConnectionDetail[];
  detailsTruncated: boolean;
  capturedAt: number;
  elapsedMs: number;
}
