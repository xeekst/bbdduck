import { useSyncExternalStore } from "react";
import { Loader2 } from "lucide-react";
import { Progress } from "@/components/ui/progress";
import { syncStore } from "@/lib/syncStore";
import { cn, formatBytes, formatSpeed } from "@/lib/utils";

export default function SyncProgressList() {
  useSyncExternalStore(syncStore.subscribe, syncStore.getSnapshot);
  const rows = syncStore.rows;
  const activeFiles = syncStore.job?.activeFiles ?? 0;

  if (rows.length === 0) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-2 text-muted-foreground">
        <Loader2
          className={cn(
            "size-6 text-muted-foreground/50",
            activeFiles > 0 && "animate-spin text-primary"
          )}
        />
        <p className="text-sm">
          {activeFiles > 0
            ? `正在等待下一次活跃快照（后端报告 ${activeFiles} 个）…`
            : "暂无正在传输的文件"}
        </p>
      </div>
    );
  }

  return (
    <div className="h-full overflow-auto overscroll-contain">
      {rows.map((row) => {
        const pct =
          row.total > 0 ? Math.min(100, (row.done / row.total) * 100) : 0;
        return (
          <div
            key={row.key}
            className="flex h-10 items-center gap-3 overflow-hidden border-b bg-primary/[0.03] px-3"
          >
            <Loader2 className="size-4 shrink-0 animate-spin text-blue-500" />
            <span
              className="min-w-0 max-w-[38%] truncate font-mono text-xs"
              title={row.path}
            >
              {row.path}
            </span>
            <Progress value={pct} className="h-1.5 min-w-8 flex-1" />
            <span className="shrink-0 text-right font-mono text-[10px] tabular-nums text-muted-foreground">
              {formatBytes(row.done)}/{formatBytes(row.total)}
              {row.total > 0 && ` · ${pct.toFixed(1)}%`}
              {row.speed > 0 && ` · ${formatSpeed(row.speed)}`}
            </span>
          </div>
        );
      })}
    </div>
  );
}
