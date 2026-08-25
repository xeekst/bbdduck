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

  if (!startedAt && logs.length === 0) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-2 text-muted-foreground">
        <Terminal className="size-6 text-muted-foreground/50" />
        <p className="text-sm">暂无同步任务日志</p>
      </div>
    );
  }

  const running = startedAt != null && finishedAt == null;
  const elapsedSec =
    startedAt != null ? Math.max(0, ((finishedAt ?? now) - startedAt) / 1000) : 0;
  const datedLogs = logs.map((l, index) => {
    const dateLabel = new Date(l.time).toLocaleDateString("zh-CN", {
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
      weekday: "short",
    });
    const previousDate = index > 0
      ? new Date(logs[index - 1].time).toLocaleDateString("zh-CN", {
          year: "numeric",
          month: "2-digit",
          day: "2-digit",
          weekday: "short",
        })
      : null;
    return { l, dateLabel, showDate: dateLabel !== previousDate };
  });
  const logFile = [...logs].reverse().find((entry) => entry.file)?.file ?? "--";

  return (
    <div className="flex h-full flex-col">
      {/* job info header */}
      <div className="flex flex-wrap items-center gap-x-5 gap-y-1.5 border-b bg-muted/20 px-3 py-2">
        <InfoRow icon={Clock} label="开始时间" value={startedAt ? formatTime(startedAt) : "--"} />
        <InfoRow icon={FolderUp} label="源目录" value={share ?? "--"} />
        <InfoRow icon={FolderDown} label="目标目录" value={localDir ?? "--"} />
        {finishedAt != null ? (
          <InfoRow icon={Clock} label="完成时间" value={formatTime(finishedAt)} />
        ) : (
          <InfoRow icon={Clock} label="状态" value={startedAt ? "进行中…" : "仅服务端日志"} />
        )}
        <InfoRow
          icon={Clock}
          label="总耗时"
          value={running ? `${formatDuration(elapsedSec)}（进行中）` : formatDuration(elapsedSec)}
        />
        <InfoRow icon={Terminal} label="日志文件" value={logFile} />
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
        {datedLogs.map(({ l, dateLabel, showDate }, i) => (
          <div key={i}>
            {showDate && <div className="sticky top-0 z-10 my-1 rounded bg-muted px-2 py-0.5 font-sans text-[10px] font-medium text-muted-foreground">{dateLabel}</div>}
            <div className="flex gap-2 whitespace-pre-wrap break-all">
              <span className="shrink-0 text-muted-foreground/60">{formatTime(l.time)}</span>
              <span className={cn("shrink-0", l.source === "server" ? "text-sky-600" : "text-emerald-600")}>
                [{l.source === "server" ? "服务端" : "客户端"}]
              </span>
              <span
                className={cn(
                  "shrink-0",
                  l.level === "error"
                    ? "text-destructive"
                    : l.level === "warn"
                      ? "text-amber-600"
                      : "text-muted-foreground"
                )}
              >
                [{l.level.toUpperCase()}]
              </span>
              <span
                className={cn(
                  l.level === "error" && "text-destructive",
                  l.level === "warn" && "text-amber-600"
                )}
              >
                {l.message}
              </span>
            </div>
          </div>
        ))}
        <div ref={bottomRef} />
      </div>
    </div>
  );
}
