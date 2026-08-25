import { useSyncExternalStore } from "react";
import { CheckCircle2, Loader2, Trash2, XCircle } from "lucide-react";
import { cn } from "@/lib/utils";
import { formatBytes, formatSpeed } from "@/lib/utils";
import { Progress } from "@/components/ui/progress";
import { syncStore } from "@/lib/syncStore";

function RowContent({
  path,
  done,
  total,
  speed,
  status,
}: {
  path: string;
  done: number;
  total: number;
  speed: number;
  status: "active" | "done" | "error" | "deleted";
}) {
  const pct = total > 0 ? Math.min(100, (done / total) * 100) : status === "done" ? 100 : 0;
  return (
    <div
      className={cn(
        "flex h-full items-center gap-3 border-b px-3",
        status === "active" && "bg-primary/[0.03]"
      )}
    >
      <div className="w-6 shrink-0">
        {status === "active" && <Loader2 className="size-4 animate-spin text-blue-500" />}
        {status === "done" && <CheckCircle2 className="size-4 text-emerald-500" />}
        {status === "error" && <XCircle className="size-4 text-red-500" />}
        {status === "deleted" && <Trash2 className="size-4 text-red-500" />}
      </div>
      <span
        className={cn(
          "min-w-0 max-w-[38%] truncate font-mono text-xs",
          status === "deleted" && "text-muted-foreground line-through"
        )}
        title={path}
      >
        {path}
      </span>
      {status === "deleted" ? (
        <span className="flex-1 pr-1 text-right text-[10px] font-medium text-destructive">
          已删除
        </span>
      ) : (
        <>
          <Progress value={pct} className="h-1.5 min-w-8 flex-1" />
          <span className="shrink-0 text-right font-mono text-[10px] tabular-nums text-muted-foreground">
            {formatBytes(done)}/{formatBytes(total)}
            {status === "active" && total > 0 && ` · ${pct.toFixed(1)}%`}
            {status === "active" && speed > 0 && ` · ${formatSpeed(speed)}`}
          </span>
        </>
      )}
    </div>
  );
}

/** Displays only files that are actively transferring. Completed files are
 * available in the completed-directory tab, so they must not bury live rows. */
export default function SyncProgressList() {
  useSyncExternalStore(syncStore.subscribe, syncStore.getSnapshot);
  const rows = syncStore.rows.filter((row) => row.status === "active");
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
            ? `正在建立 ${activeFiles} 个文件的传输进度…`
            : "暂无正在传输的文件"}
        </p>
      </div>
    );
  }

  return (
    <div className="h-full overflow-auto overscroll-contain">
      {rows.map((row) => (
        <div key={row.key} className="h-10 overflow-hidden">
          <RowContent
            path={row.path}
            done={row.done}
            total={row.total}
            speed={row.speed}
            status={row.status}
          />
        </div>
      ))}
    </div>
  );
}
