export interface PortEndpoint {
  protocol: "TCP" | "UDP";
  addressFamily: "IPv4" | "IPv6";
  localIp: string;
  localPort: number;
  remoteIp: string | null;
  remotePort: number | null;
  state: string;
  listening: boolean;
  wildcard: boolean;
}

export interface ProcessTreeNode {
  pid: number;
  parentPid: number;
  name: string;
  threadCount: number;
  isTarget: boolean;
  children: ProcessTreeNode[];
}

export interface PortOccupyingProcess {
  pid: number;
  parentPid: number;
  name: string;
  path: string | null;
  commandLine: string | null;
  appType: "critical" | "regular";
  sessionId: number;
  startedAt: number | null;
  threadCount: number;
  endpoints: PortEndpoint[];
  parentChain: ProcessTreeNode[];
  processTree: ProcessTreeNode;
  treeTruncated: boolean;
}

export interface PortOccupancyScanResult {
  port: number;
  occupied: boolean;
  listenerCount: number;
  endpointCount: number;
  elapsedMs: number;
  processes: PortOccupyingProcess[];
}
