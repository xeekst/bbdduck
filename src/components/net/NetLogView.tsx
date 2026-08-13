import { useEffect, useRef } from "react";
import { Terminal } from "lucide-react";
import { cn, formatTime } from "@/lib/utils";
import type { NetLogEvent } from "@/lib/net-types";

interface Props {
  logs: NetLogEvent[];
  emptyText?: string;
  /** 日志框标题，默认「网络日志」 */
  title?: string;
}

export default function NetLogView({
  logs,
  emptyText = "暂无日志",
  title = "网络日志",
}: Props) {
  const bottomRef = useRef<HTMLDivElement>(null);
  const atBottomRef = useRef(true);

  // 新日志到来时若在底部则自动滚动到底部
  useEffect(() => {
    if (atBottomRef.current) {
      bottomRef.current?.scrollIntoView({ block: "end" });
    }
  }, [logs.length]);

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex items-center gap-1.5 border-b bg-muted/30 px-3 py-1.5 text-[11px] font-medium text-muted-foreground">
        <Terminal className="size-3.5" />
        {title}
        <span className="ml-auto text-[10px] text-muted-foreground/60">
          {logs.length} 条
        </span>
      </div>
      <div
        className="min-h-0 flex-1 overflow-auto p-2 font-mono text-[11px] leading-5"
        onScroll={(e) => {
          const el = e.currentTarget;
          atBottomRef.current = el.scrollTop + el.clientHeight >= el.scrollHeight - 16;
        }}
      >
        {logs.length === 0 && (
          <div className="py-6 text-center text-muted-foreground">{emptyText}</div>
        )}
        {logs.map((l, i) => (
          <div key={i} className="flex gap-2 whitespace-pre-wrap break-all">
            <span className="shrink-0 text-muted-foreground/60">
              {formatTime(l.time * 1000)}
            </span>
            <span
              className={cn(
                "shrink-0",
                l.level === "error" ? "text-destructive" : "text-muted-foreground"
              )}
            >
              [{l.level.toUpperCase()}]
            </span>
            <span className={cn(l.level === "error" && "text-destructive")}>
              {l.message}
            </span>
          </div>
        ))}
        <div ref={bottomRef} />
      </div>
    </div>
  );
}
