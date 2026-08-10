import { useEffect, useLayoutEffect, useRef, useState, useSyncExternalStore } from "react";
import { CheckCircle2, Loader2, Trash2, XCircle } from "lucide-react";
import { cn } from "@/lib/utils";
import { formatBytes, formatSpeed } from "@/lib/utils";
import { Progress } from "@/components/ui/progress";
import { syncStore } from "@/lib/syncStore";

const ITEM_H = 40;
const BUFFER = 12;

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

/**
 * A virtualized, windowed list of transfer rows. Only the rows inside the
 * viewport (+ buffer) are rendered, so it stays fast even with tens of
 * thousands of entries. Scroll up to inspect older rows; new rows keep the
 * view pinned to the bottom while you are at the bottom.
 */
export default function SyncProgressList() {
  useSyncExternalStore(syncStore.subscribe, syncStore.getSnapshot);
  const rows = syncStore.rows;

  const containerRef = useRef<HTMLDivElement>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewportH, setViewportH] = useState(400);
  const atBottomRef = useRef(true);
  const prevCountRef = useRef(0);

  // Keep pinned to bottom when new rows arrive if the user was at the bottom.
  useLayoutEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    if (rows.length > prevCountRef.current && atBottomRef.current) {
      el.scrollTop = el.scrollHeight;
    }
    prevCountRef.current = rows.length;
  }, [rows.length, scrollTop]);

  // Measure viewport height.
  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => {
      setViewportH(el.clientHeight);
    });
    ro.observe(el);
    setViewportH(el.clientHeight);
    return () => ro.disconnect();
  }, []);

  const onScroll = (e: React.UIEvent<HTMLDivElement>) => {
    const el = e.currentTarget;
    setScrollTop(el.scrollTop);
    atBottomRef.current = el.scrollTop + el.clientHeight >= el.scrollHeight - 12;
  };

  const total = rows.length * ITEM_H;
  const start = Math.max(0, Math.floor(scrollTop / ITEM_H) - BUFFER);
  const end = Math.min(rows.length, Math.ceil((scrollTop + viewportH) / ITEM_H) + BUFFER);

  const visible: { i: number; row: (typeof rows)[number] }[] = [];
  for (let i = start; i < end; i++) {
    visible.push({ i, row: rows[i] });
  }

  if (rows.length === 0) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-2 text-muted-foreground">
        <Loader2 className="size-6 text-muted-foreground/50" />
        <p className="text-sm">暂无传输任务，等待开始同步…</p>
      </div>
    );
  }

  return (
    <div
      ref={containerRef}
      onScroll={onScroll}
      className="h-full overflow-auto overscroll-contain"
    >
      <div className="relative" style={{ height: total }}>
        {visible.map(({ i, row }) => (
          <div
            key={row.key}
            className="absolute left-0 right-0 overflow-hidden"
            style={{ top: i * ITEM_H, height: ITEM_H }}
          >
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
    </div>
  );
}
