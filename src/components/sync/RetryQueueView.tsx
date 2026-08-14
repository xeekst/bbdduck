import { useEffect, useState, useSyncExternalStore } from "react";
import { RefreshCw, RotateCcw } from "lucide-react";
import { syncStore } from "@/lib/syncStore";

/**
 * Shows files that failed and are queued for automatic retry (up to 3 attempts,
 * 3 seconds apart). Items disappear once a retry succeeds or retries are
 * exhausted (the failure then shows in the transfer list and log tab).
 */
export default function RetryQueueView() {
  useSyncExternalStore(syncStore.subscribe, syncStore.getSnapshot);
  const retries = [...syncStore.retries.values()];
  const [now, setNow] = useState(Date.now());

  useEffect(() => {
    if (retries.length === 0) return;
    const t = setInterval(() => setNow(Date.now()), 500);
    return () => clearInterval(t);
  }, [retries.length]);

  if (retries.length === 0) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-2 text-muted-foreground">
        <RotateCcw className="size-6 text-muted-foreground/50" />
        <p className="text-sm">重试队列为空</p>
      </div>
    );
  }

  return (
    <div className="h-full overflow-auto p-2">
      {retries.map((r) => {
        const remain = Math.max(0, (r.retryAt - now) / 1000);
        return (
          <div
            key={r.path}
            className="flex items-center gap-2 border-b bg-amber-500/[0.04] px-2 py-1.5 text-xs"
          >
            <RefreshCw className="size-3.5 shrink-0 animate-spin text-amber-500" />
            <span className="min-w-0 flex-1 truncate font-mono" title={r.path}>
              {r.path}
            </span>
            <span className="shrink-0 text-muted-foreground">
              第 {r.attempt}/{r.maxRetries} 次重试
            </span>
            <span className="w-10 shrink-0 text-right font-mono tabular-nums text-amber-600">
              {remain <= 0 ? "重试中" : `${Math.ceil(remain)}s`}
            </span>
          </div>
        );
      })}
    </div>
  );
}
