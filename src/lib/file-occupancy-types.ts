export interface MatchedFileHandle {
  handleValue: string;
  path: string;
  grantedAccess: string;
}

export interface OccupyingProcess {
  pid: number;
  processToken: string;
  name: string;
  path: string | null;
  appType: "regular" | "critical";
  sessionId: number;
  startedAt: number | null;
  canTerminate: boolean;
  handles: MatchedFileHandle[];
  handleCount: number;
}

export interface OccupancyScanResult {
  query: string;
  scannedHandles: number;
  fileHandles: number;
  matchedHandles: number;
  inaccessibleProcesses: number;
  unresolvedHandles: number;
  truncated: boolean;
  elapsedMs: number;
  processes: OccupyingProcess[];
}

