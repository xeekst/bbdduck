import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { Clock, FolderDown, FolderUp, Terminal } from "lucide-react";
import { cn, formatDuration, formatTime } from "@/lib/utils";
import { syncStore } from "@/lib/syncStore";

function InfoRow({ icon: Icon, label, value }: { icon?: React.ComponentType<{ className?: string }>; label: string; value: string }) {
  return (
    <div className="flex items-center gap-1.5 text-[11px]">
      {Icon && <Icon className="size-3.5 shrink-0 text-muted-foreground" />}
      <span className="shrink-0 text-muted-foreground">{label}</span>
      <span className="max-w-[260px] truncate font-mono" title={value}>
        {value}
      </span>
    </div>
  );
}

export default function SyncLogView() {
  useSyncExternalStore(syncStore.subscribe, syncStore.getSnapshot);
  const logs = syncStore.logs;
  const { share, localDir, startedAt, finishedAt } = syncStore;
  const [now, setNow] = useState(Date.now());

  const bottomRef = useRef<HTMLDivElement>(null);
  const atBottomRef = useRef(true);

  // tick every second while a job is running to update the live elapsed time
  useEffect(() => {
    if (!startedAt || finishedAt != null) return;
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, [startedAt, finishedAt]);

  useEffect(() => {
    if (atBottomRef.current) {
      bottomRef.current?.scrollIntoView({ block: "end" });
    }
  }, [logs.length]);

  if (!startedAt) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-2 text-muted-foreground">
        <Terminal className="size-6 text-muted-foreground/50" />
        <p className="text-sm">暂无同步任务日志</p>
      </div>
    );
  }

  const running = finishedAt == null;
  const elapsedSec =
    startedAt != null ? Math.max(0, ((finishedAt ?? now) - startedAt) / 1000) : 0;

  return (
    <div className="flex h-full flex-col">
      {/* job info header */}
      <div className="flex flex-wrap items-center gap-x-5 gap-y-1.5 border-b bg-muted/20 px-3 py-2">
        <InfoRow icon={Clock} label="开始时间" value={formatTime(startedAt)} />
        <InfoRow icon={FolderUp} label="源目录" value={share ?? "--"} />
        <InfoRow icon={FolderDown} label="目标目录" value={localDir ?? "--"} />
        {finishedAt != null ? (
          <InfoRow icon={Clock} label="完成时间" value={formatTime(finishedAt)} />
        ) : (
          <InfoRow icon={Clock} label="状态" value="进行中…" />
        )}
        <InfoRow
          icon={Clock}
          label="总耗时"
          value={running ? `${formatDuration(elapsedSec)}（进行中）` : formatDuration(elapsedSec)}
        />
      </div>

      {/* log lines */}
      <div
        className="min-h-0 flex-1 overflow-auto p-2 font-mono text-[11px] leading-5"
        onScroll={(e) => {
          const el = e.currentTarget;
          atBottomRef.current = el.scrollTop + el.clientHeight >= el.scrollHeight - 16;
        }}
      >
        {logs.length === 0 && (
          <div className="py-6 text-center text-muted-foreground">暂无日志</div>
        )}
        {logs.map((l, i) => (
          <div key={i} className="flex gap-2 whitespace-pre-wrap break-all">
            <span className="shrink-0 text-muted-foreground/60">{formatTime(l.time)}</span>
            <span
              className={cn(
                "shrink-0",
                l.level === "error" ? "text-destructive" : "text-muted-foreground"
              )}
            >
              [{l.level.toUpperCase()}]
            </span>
            <span className={cn(l.level === "error" && "text-destructive")}>{l.message}</span>
          </div>
        ))}
        <div ref={bottomRef} />
      </div>
    </div>
  );
}
