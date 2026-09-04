import {
  useCallback,
  useEffect,
  useState,
  useSyncExternalStore,
} from "react";
import {
  ChevronLeft,
  ChevronRight,
  File as FileIcon,
  Folder,
  FolderOpen,
  Loader2,
  RefreshCw,
} from "lucide-react";
import { api } from "@/lib/api";
import type { CompletedPage } from "@/lib/sync-types";
import { syncStore } from "@/lib/syncStore";
import { formatBytes } from "@/lib/utils";
import { Button } from "@/components/ui/button";

const PAGE_SIZE = 200;

/**
 * Browses the local target directory on demand. No completed file path is kept
 * in WebView memory, so the cost is independent of total sync file count.
 */
export default function CompletedTreeView() {
  useSyncExternalStore(syncStore.subscribe, syncStore.getSnapshot);
  const root = syncStore.localDir;
  const doneFiles = syncStore.job?.doneFiles ?? 0;
  const [relative, setRelative] = useState("");
  const [offset, setOffset] = useState(0);
  const [page, setPage] = useState<CompletedPage | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [refreshToken, setRefreshToken] = useState(0);

  useEffect(() => {
    setRelative("");
    setOffset(0);
    setPage(null);
  }, [root]);

  useEffect(() => {
    if (!root) return;
    let cancelled = false;
    setLoading(true);
    setError(null);
    api
      .syncListCompleted(root, relative, offset, PAGE_SIZE)
      .then((result) => {
        if (!cancelled) setPage(result);
      })
      .catch((reason) => {
        if (!cancelled) {
          setPage(null);
          setError(String(reason));
        }
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [root, relative, offset, refreshToken]);

  const openDirectory = useCallback((path: string) => {
    setRelative(path);
    setOffset(0);
  }, []);

  if (!root) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-2 text-muted-foreground">
        <FolderOpen className="size-6 text-muted-foreground/50" />
        <p className="text-sm">开始同步后可按需浏览本地完成目录</p>
      </div>
    );
  }

  const parts = relative.split("/").filter(Boolean);
  const rangeStart = page && page.entries.length > 0 ? page.offset + 1 : 0;
  const rangeEnd = page ? page.offset + page.entries.length : 0;

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex flex-wrap items-center gap-2 border-b px-3 py-2 text-xs">
        <span className="text-muted-foreground">
          已完成 <span className="font-medium text-foreground">{doneFiles.toLocaleString()}</span>{" "}
          个文件
        </span>
        <span className="text-muted-foreground/60">·</span>
        <button
          className="font-mono text-primary hover:underline"
          onClick={() => openDirectory("")}
          title={root}
        >
          根目录
        </button>
        {parts.map((part, index) => {
          const path = parts.slice(0, index + 1).join("/");
          return (
            <span key={path} className="flex min-w-0 items-center gap-1">
              <ChevronRight className="size-3 text-muted-foreground" />
              <button
                className="max-w-48 truncate font-mono hover:text-primary hover:underline"
                onClick={() => openDirectory(path)}
                title={path}
              >
                {part}
              </button>
            </span>
          );
        })}
        <Button
          variant="ghost"
          size="icon"
          className="ml-auto size-7"
          onClick={() => setRefreshToken((value) => value + 1)}
          disabled={loading}
          title="刷新当前目录"
        >
          <RefreshCw className={loading ? "animate-spin" : ""} />
        </Button>
      </div>

      <div className="min-h-0 flex-1 overflow-auto">
        {loading && !page ? (
          <div className="flex h-full items-center justify-center gap-2 text-sm text-muted-foreground">
            <Loader2 className="size-4 animate-spin" /> 正在读取本地目录…
          </div>
        ) : error ? (
          <div className="flex h-full items-center justify-center px-4 text-sm text-destructive">
            {error}
          </div>
        ) : page && page.entries.length > 0 ? (
          page.entries.map((entry) => (
            <button
              key={entry.path}
              className="grid w-full grid-cols-[minmax(0,1fr)_120px_180px] items-center gap-3 border-b px-3 py-2 text-left text-xs hover:bg-accent/60 disabled:pointer-events-none"
              onClick={() => entry.isDir && openDirectory(entry.path)}
              disabled={!entry.isDir}
              title={entry.path}
            >
              <span className="flex min-w-0 items-center gap-2">
                {entry.isDir ? (
                  <Folder className="size-4 shrink-0 text-amber-500" />
                ) : (
                  <FileIcon className="size-4 shrink-0 text-muted-foreground" />
                )}
                <span className="truncate font-mono">{entry.name}</span>
              </span>
              <span className="text-right font-mono text-muted-foreground">
                {entry.isDir ? "目录" : formatBytes(entry.size)}
              </span>
              <span className="text-right font-mono text-muted-foreground">
                {entry.modified
                  ? new Date(entry.modified * 1000).toLocaleString()
                  : "—"}
              </span>
            </button>
          ))
        ) : (
          <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
            当前目录为空
          </div>
        )}
      </div>

      <div className="flex items-center border-t px-3 py-2 text-xs text-muted-foreground">
        <span>
          当前显示 {rangeStart.toLocaleString()}–{rangeEnd.toLocaleString()}；每页最多 {PAGE_SIZE} 条
        </span>
        <span className="ml-3">目录内容按需读取，不常驻前端内存</span>
        <div className="ml-auto flex gap-1">
          <Button
            variant="outline"
            size="sm"
            onClick={() => setOffset((value) => Math.max(0, value - PAGE_SIZE))}
            disabled={loading || offset === 0}
          >
            <ChevronLeft /> 上一页
          </Button>
          <Button
            variant="outline"
            size="sm"
            onClick={() => setOffset((value) => value + PAGE_SIZE)}
            disabled={loading || !page?.hasMore}
          >
            下一页 <ChevronRight />
          </Button>
        </div>
      </div>
    </div>
  );
}
