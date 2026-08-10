import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { Badge } from "@/components/ui/badge";
import { Separator } from "@/components/ui/separator";
import { formatBytes, formatSpeed } from "@/lib/utils";
import {
  EVT_JOB,
  EVT_SERVER,
  type JobEvent,
  type ServerEvent,
} from "@/lib/sync-types";
import { api } from "@/lib/api";

interface StatusState {
  server: ServerEvent | null;
  job: JobEvent | null;
}

function StatusBar() {
  const [state, setState] = useState<StatusState>({ server: null, job: null });
  // Track the job id and whether it already reached a terminal state, so a
  // late "running" event for the same (stopped) job cannot re-light "同步中".
  const jobIdRef = useRef<string | null>(null);
  const terminalRef = useRef(false);

  useEffect(() => {
    let disposed = false;
    const unlisten: (() => void)[] = [];

    (async () => {
      const [s, j] = await Promise.all([
        listen<ServerEvent>(EVT_SERVER, (e) => {
          if (!disposed) setState((prev) => ({ ...prev, server: e.payload }));
        }),
        listen<JobEvent>(EVT_JOB, (e) => {
          if (disposed) return;
          const job = e.payload;
          if (job.status === "running") {
            if (terminalRef.current && jobIdRef.current === job.id) return;
            jobIdRef.current = job.id;
            terminalRef.current = false;
            setState((prev) => ({ ...prev, job }));
          } else {
            terminalRef.current = true;
            setState((prev) => ({ ...prev, job: null }));
          }
        }),
      ]);
      unlisten.push(s, j);

      // initial snapshot
      try {
        const status = await api.serverStatus();
        if (!disposed)
          setState((prev) => ({
            ...prev,
            server: { running: status.running, addr: status.addr, shares: status.shares },
          }));
      } catch {
        /* ignore */
      }
    })();

    return () => {
      disposed = true;
      unlisten.forEach((fn) => fn());
    };
  }, []);

  const { server, job } = state;

  return (
    <footer className="flex h-7 shrink-0 items-center gap-3 border-t bg-muted/40 px-3 text-xs text-muted-foreground">
      {/* Server / connection status */}
      <div className="flex items-center gap-1.5">
        <span
          className={
            "inline-block size-2 rounded-full " +
            (server?.running ? "bg-emerald-500" : "bg-zinc-400")
          }
        />
        <span className="font-medium">
          {server?.running ? `监听中 ${server.addr ?? ""}` : "未监听"}
        </span>
        {server?.running && (
          <span className="text-[10px]">共享 {server.shares?.length ?? 0} 个文件夹</span>
        )}
      </div>

      <Separator orientation="vertical" className="h-3.5" />

      {/* Active sync job */}
      {job ? (
        <div className="flex min-w-0 items-center gap-1.5">
          <span className="inline-block size-2 animate-pulse rounded-full bg-blue-500" />
          <span className="truncate">
            同步中 · {job.doneFiles}/{job.totalFiles} 个文件 ·{" "}
            {formatBytes(job.doneBytes)}/{formatBytes(job.totalBytes)} ·{" "}
            {formatSpeed(job.speed)}
          </span>
        </div>
      ) : (
        <span>空闲</span>
      )}

      <div className="ml-auto flex items-center gap-1.5">
        {job?.status === "running" && (
          <Badge variant="secondary" className="text-[10px]">
            正在同步
          </Badge>
        )}
        <Badge variant="outline" className="text-[10px]">
          SQLite · 本地存储
        </Badge>
      </div>
    </footer>
  );
}

export default StatusBar;
