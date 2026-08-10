import { useEffect, useState, useSyncExternalStore } from "react";
import { CheckCircle2, CircleStop, Loader2, XCircle } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { formatBytes, formatDuration, formatSpeed } from "@/lib/utils";
import { syncStore } from "@/lib/syncStore";

export default function SyncSummaryBar({ startedAt }: { startedAt: number | null }) {
  useSyncExternalStore(syncStore.subscribe, syncStore.getSnapshot);
  const job = syncStore.job;
  const [now, setNow] = useState(Date.now());

  useEffect(() => {
    // Only tick the elapsed-time clock while a job is actually running.
    if (!job || job.status !== "running") return;
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, [job?.status]);

  if (!job) {
    return (
      <div className="flex items-center gap-2 text-xs text-muted-foreground">
        <CircleStop className="size-4" />
        尚未开始同步。配置好“节点 A”与“节点 B”后点击“开始同步”。
      </div>
    );
  }

  const running = job.status === "running";
  const elapsed = startedAt ? Math.max(0, (now - startedAt) / 1000) : 0;

  return (
    <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs">
      <Badge
        variant={
          job.status === "running"
            ? "default"
            : job.status === "finished"
              ? "secondary"
              : "destructive"
        }
      >
        {running ? (
          <Loader2 className="size-3 animate-spin" />
        ) : job.status === "finished" ? (
          <CheckCircle2 className="size-3" />
        ) : (
          <XCircle className="size-3" />
        )}
        {job.status === "running" && "同步中"}
        {job.status === "finished" && "已完成"}
        {job.status === "stopped" && "已停止"}
        {job.status === "error" && "出错"}
      </Badge>

      <span>
        文件 <b className="font-mono">{job.doneFiles.toLocaleString()}</b> /{" "}
        {job.totalFiles.toLocaleString()}
      </span>
      {job.skippedFiles > 0 && <span className="text-muted-foreground">跳过 {job.skippedFiles.toLocaleString()}</span>}
      {job.failedFiles > 0 && <span className="text-destructive">失败 {job.failedFiles.toLocaleString()}</span>}
      <span>
        数据 <b className="font-mono">{formatBytes(job.doneBytes)}</b> / {formatBytes(job.totalBytes)}
      </span>
      <span className="font-mono text-primary">{formatSpeed(job.speed)}</span>
      {running && <span>已用 {formatDuration(elapsed)}</span>}
      {job.message && <span className="max-w-[40%] truncate text-amber-600">{job.message}</span>}
    </div>
  );
}
