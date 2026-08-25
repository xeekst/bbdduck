import { type FormEvent, useState } from "react";
import {
  AlertTriangle,
  CheckCircle2,
  Clock3,
  FileSearch,
  Loader2,
  RefreshCw,
  Search,
  ShieldAlert,
  Square,
} from "lucide-react";
import { api } from "@/lib/api";
import type {
  OccupancyScanResult,
  OccupyingProcess,
} from "@/lib/file-occupancy-types";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";

function errorText(error: unknown) {
  return typeof error === "string"
    ? error
    : error instanceof Error
      ? error.message
      : "操作失败，请重试";
}

function formatStartedAt(seconds: number | null) {
  if (!seconds) return "无法读取";
  return new Intl.DateTimeFormat("zh-CN", {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  }).format(new Date(seconds * 1000));
}

function fuzzyMatchIndexes(text: string, query: string) {
  const chars = Array.from(text);
  const needle = Array.from(query.trim().toLocaleLowerCase());
  const matched = new Set<number>();
  if (needle.length === 0) return matched;

  let componentStart = 0;
  for (let boundary = 0; boundary <= chars.length; boundary += 1) {
    const separator =
      boundary === chars.length ||
      chars[boundary] === "\\" ||
      chars[boundary] === "/";
    if (!separator) continue;

    let queryIndex = 0;
    const componentMatches: number[] = [];
    for (
      let index = componentStart;
      index < boundary && queryIndex < needle.length;
      index += 1
    ) {
      if (chars[index].toLocaleLowerCase() === needle[queryIndex]) {
        componentMatches.push(index);
        queryIndex += 1;
      }
    }
    if (queryIndex === needle.length) {
      componentMatches.forEach((index) => matched.add(index));
    }
    componentStart = boundary + 1;
  }
  return matched;
}

function HighlightedPath({ path, query }: { path: string; query: string }) {
  const chars = Array.from(path);
  const matches = fuzzyMatchIndexes(path, query);

  return (
    <span>
      {chars.map((char, index) =>
        matches.has(index) ? (
          <mark
            key={index}
            className="rounded-sm bg-amber-300/80 px-px text-amber-950 dark:bg-amber-400/70 dark:text-amber-950"
          >
            {char}
          </mark>
        ) : (
          <span key={index}>{char}</span>
        )
      )}
    </span>
  );
}

function ProcessCard({
  process,
  query,
  onTerminate,
}: {
  process: OccupyingProcess;
  query: string;
  onTerminate: (process: OccupyingProcess) => void;
}) {
  const critical = process.appType === "critical";

  return (
    <div className="overflow-hidden rounded-lg border bg-card shadow-xs">
      <div className="flex items-start gap-3 border-b p-4">
        <div className="flex size-10 shrink-0 items-center justify-center rounded-lg bg-primary/8">
          <Square className="size-4 fill-primary/15 text-primary" />
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <h3 className="truncate text-sm font-semibold">{process.name}</h3>
            <Badge variant="secondary" className="font-mono font-normal">
              PID {process.pid}
            </Badge>
            <Badge
              variant="outline"
              className={
                critical
                  ? "border-destructive/30 bg-destructive/5 text-destructive"
                  : "border-emerald-500/30 bg-emerald-500/8 text-emerald-700"
              }
            >
              {critical ? "Windows 关键进程" : "运行中"}
            </Badge>
          </div>
          <p
            className="mt-1 truncate font-mono text-[11px] text-muted-foreground"
            title={process.path ?? undefined}
          >
            {process.path ?? "进程路径不可读（可能需要管理员权限）"}
          </p>
        </div>
        <Button
          variant="destructive"
          size="sm"
          disabled={!process.canTerminate}
          onClick={() => onTerminate(process)}
          title={
            process.canTerminate
              ? "强制终止此进程"
              : "Windows 关键进程或当前应用不可终止"
          }
        >
          <ShieldAlert className="size-3.5" />
          终止进程
        </Button>
      </div>

      <div className="grid grid-cols-2 gap-px bg-border sm:grid-cols-4">
        {[
          ["进程 PID", String(process.pid)],
          ["启动时间", formatStartedAt(process.startedAt)],
          ["会话 ID", String(process.sessionId)],
          ["匹配 Handle", String(process.handleCount)],
        ].map(([label, value]) => (
          <div key={label} className="min-w-0 bg-card px-3 py-2.5">
            <p className="text-[10px] text-muted-foreground">{label}</p>
            <p className="mt-0.5 truncate text-xs font-medium" title={value}>
              {value}
            </p>
          </div>
        ))}
      </div>

      <div className="border-t bg-muted/20 p-3">
        <p className="mb-2 text-[11px] font-medium text-muted-foreground">
          匹配到的打开 Handle
        </p>
        <div className="space-y-1.5">
          {process.handles.map((handle) => (
            <div
              key={`${handle.handleValue}-${handle.path}`}
              className="rounded-md border bg-background px-2.5 py-2"
            >
              <div className="mb-1.5 flex flex-wrap items-center gap-1.5">
                <Badge variant="secondary" className="font-mono text-[10px]">
                  Handle {handle.handleValue}
                </Badge>
                <Badge variant="outline" className="font-mono text-[10px] font-normal">
                  Access {handle.grantedAccess}
                </Badge>
              </div>
              <p className="break-all font-mono text-[11px] leading-5">
                <HighlightedPath path={handle.path} query={query} />
              </p>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

function FileOccupancyPage() {
  const [query, setQuery] = useState("");
  const [result, setResult] = useState<OccupancyScanResult | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [selectedProcess, setSelectedProcess] =
    useState<OccupyingProcess | null>(null);
  const [terminating, setTerminating] = useState(false);
  const [terminateError, setTerminateError] = useState<string | null>(null);

  const scan = async (value = query, keepNotice = false) => {
    const keyword = value.trim();
    if (!keyword) {
      setError("请输入文件或文件夹名称");
      return;
    }
    setQuery(keyword);
    setLoading(true);
    setError(null);
    if (!keepNotice) setNotice(null);
    try {
      setResult(await api.fileOccupancyScan(keyword));
    } catch (scanError) {
      setResult(null);
      setError(errorText(scanError));
    } finally {
      setLoading(false);
    }
  };

  const submit = (event: FormEvent) => {
    event.preventDefault();
    void scan();
  };

  const openTerminateDialog = (process: OccupyingProcess) => {
    setTerminateError(null);
    setSelectedProcess(process);
  };

  const closeTerminateDialog = () => {
    if (terminating) return;
    setSelectedProcess(null);
    setTerminateError(null);
  };

  const terminateSelected = async () => {
    if (!selectedProcess || terminating) return;
    setTerminating(true);
    setTerminateError(null);
    try {
      await api.fileOccupancyTerminate(
        selectedProcess.pid,
        selectedProcess.processToken
      );
      const name = selectedProcess.name;
      setSelectedProcess(null);
      setNotice(`已终止 ${name}，Handle 搜索结果已刷新`);
      await scan(result?.query ?? query, true);
    } catch (terminateFailure) {
      setTerminateError(errorText(terminateFailure));
    } finally {
      setTerminating(false);
    }
  };

  const hasProcesses = (result?.processes.length ?? 0) > 0;
  const hasWarnings =
    !!result &&
    (result.truncated ||
      result.inaccessibleProcesses > 0 ||
      result.unresolvedHandles > 0);

  return (
    <div className="flex h-full flex-col gap-4 overflow-hidden p-4">
      <div className="flex shrink-0 items-center gap-2">
        <FileSearch className="size-5 text-primary" />
        <h1 className="text-lg font-semibold">文件占用检测</h1>
        <p className="text-xs text-muted-foreground">
          按名称模糊搜索系统中已打开的文件与文件夹 Handle
        </p>
      </div>

      <Card className="shrink-0 gap-3 py-4">
        <CardContent className="px-4">
          <form className="flex flex-col gap-2 sm:flex-row" onSubmit={submit}>
            <div className="relative min-w-0 flex-1">
              <Search className="absolute left-3 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
              <Input
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                placeholder="输入文件或文件夹名称，如 report、node_modules"
                className="pl-9 text-sm"
                disabled={loading}
                autoFocus
              />
            </div>
            <Button type="submit" disabled={loading || !query.trim()}>
              {loading ? (
                <Loader2 className="size-4 animate-spin" />
              ) : (
                <Search className="size-4" />
              )}
              {loading ? "正在枚举 Handle" : "模糊搜索"}
            </Button>
          </form>
          <p className="mt-2 text-[11px] text-muted-foreground">
            搜索当前系统已打开的磁盘文件 Handle；字符按顺序匹配且不区分大小写，例如
            “rpt”可以匹配“report”。
          </p>

          {error && (
            <div className="mt-3 flex items-start gap-2 rounded-md border border-destructive/30 bg-destructive/5 px-3 py-2 text-xs text-destructive">
              <AlertTriangle className="mt-0.5 size-3.5 shrink-0" />
              <span>{error}</span>
            </div>
          )}
          {notice && !error && (
            <div className="mt-3 flex items-start gap-2 rounded-md border border-emerald-500/30 bg-emerald-500/5 px-3 py-2 text-xs text-emerald-700">
              <CheckCircle2 className="mt-0.5 size-3.5 shrink-0" />
              <span>{notice}</span>
            </div>
          )}
        </CardContent>
      </Card>

      {result && (
        <div className="grid shrink-0 grid-cols-2 gap-2 lg:grid-cols-4">
          {[
            ["匹配 Handle", result.matchedHandles.toLocaleString("zh-CN")],
            ["相关进程", result.processes.length.toLocaleString("zh-CN")],
            ["已枚举文件 Handle", result.fileHandles.toLocaleString("zh-CN")],
            ["搜索耗时", `${result.elapsedMs.toLocaleString("zh-CN")} ms`],
          ].map(([label, value]) => (
            <div key={label} className="rounded-lg border bg-card px-3 py-2.5 shadow-xs">
              <p className="text-[10px] text-muted-foreground">{label}</p>
              <p className="mt-0.5 text-sm font-semibold">{value}</p>
            </div>
          ))}
        </div>
      )}

      <Card className="min-h-0 flex-1 gap-0 overflow-hidden py-0">
        <CardHeader className="flex-row items-center border-b px-4 py-3 !pb-3">
          <div className="min-w-0 flex-1">
            <CardTitle className="text-sm">Handle 搜索结果</CardTitle>
            <p className="mt-1 truncate text-[11px] text-muted-foreground">
              {result ? (
                <>
                  模糊条件：
                  <span className="font-mono font-medium text-foreground">
                    {result.query}
                  </span>
                  <span className="ml-2">
                    共枚举 {result.scannedHandles.toLocaleString("zh-CN")} 个系统 Handle
                  </span>
                </>
              ) : (
                "尚未输入搜索名称"
              )}
            </p>
          </div>
          {result && (
            <Button
              variant="ghost"
              size="sm"
              onClick={() => void scan(result.query)}
              disabled={loading}
            >
              <RefreshCw className={loading ? "size-3.5 animate-spin" : "size-3.5"} />
              刷新
            </Button>
          )}
        </CardHeader>

        <CardContent className="min-h-0 flex-1 p-0">
          {loading ? (
            <div className="flex h-full flex-col items-center justify-center gap-3 text-muted-foreground">
              <Loader2 className="size-8 animate-spin text-primary" />
              <div className="text-center">
                <p className="text-sm font-medium text-foreground">
                  正在枚举系统文件 Handle
                </p>
                <p className="mt-1 text-xs">
                  正在读取各进程打开的文件和文件夹路径
                </p>
              </div>
            </div>
          ) : !result ? (
            <div className="flex h-full flex-col items-center justify-center gap-3 text-muted-foreground">
              <div className="flex size-14 items-center justify-center rounded-full bg-muted">
                <FileSearch className="size-6" />
              </div>
              <div className="text-center">
                <p className="text-sm font-medium text-foreground">
                  输入名称开始搜索
                </p>
                <p className="mt-1 text-xs">
                  无需完整路径，匹配字符会在每条 Handle 路径中高亮
                </p>
              </div>
            </div>
          ) : !hasProcesses ? (
            <div className="flex h-full flex-col items-center justify-center gap-3 text-muted-foreground">
              <div className="flex size-14 items-center justify-center rounded-full bg-emerald-500/10">
                <CheckCircle2 className="size-7 text-emerald-600" />
              </div>
              <div className="text-center">
                <p className="text-sm font-medium text-foreground">
                  没有匹配的打开 Handle
                </p>
                <p className="mt-1 text-xs">
                  当前没有进程打开名称模糊匹配“{result.query}”的文件或文件夹
                </p>
              </div>
              {hasWarnings && (
                <div className="max-w-xl rounded-md border border-amber-500/30 bg-amber-500/5 px-3 py-2 text-center text-xs text-amber-700">
                  部分受保护进程或 Handle 无法读取；尝试以管理员身份运行可获得更多结果。
                </div>
              )}
            </div>
          ) : (
            <ScrollArea className="h-full">
              <div className="space-y-3 p-4">
                {hasWarnings && (
                  <div className="flex gap-2 rounded-md border border-amber-500/30 bg-amber-500/5 px-3 py-2 text-xs text-amber-700">
                    <AlertTriangle className="mt-0.5 size-3.5 shrink-0" />
                    <span>
                      {result.truncated && "匹配结果超过 500 个，已截断显示。 "}
                      {result.inaccessibleProcesses > 0 &&
                        `${result.inaccessibleProcesses} 个受保护进程无法访问。 `}
                      {result.unresolvedHandles > 0 &&
                        `${result.unresolvedHandles} 个 Handle 无法解析路径。`}
                    </span>
                  </div>
                )}
                {result.processes.map((process) => (
                  <ProcessCard
                    key={process.processToken}
                    process={process}
                    query={result.query}
                    onTerminate={openTerminateDialog}
                  />
                ))}
              </div>
            </ScrollArea>
          )}
        </CardContent>
      </Card>

      <Dialog
        open={selectedProcess !== null}
        onOpenChange={(open) => {
          if (!open) closeTerminateDialog();
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>确认终止进程？</DialogTitle>
            <DialogDescription>
              强制终止可能导致该进程中未保存的数据丢失。操作完成后会自动刷新 Handle 搜索结果。
            </DialogDescription>
          </DialogHeader>
          {selectedProcess && (
            <div className="rounded-md border bg-muted/40 p-3 text-xs">
              <div className="flex items-center gap-2">
                <span className="font-semibold">{selectedProcess.name}</span>
                <Badge variant="secondary" className="font-mono">
                  PID {selectedProcess.pid}
                </Badge>
              </div>
              <p className="mt-2 break-all font-mono text-[11px] text-muted-foreground">
                {selectedProcess.path ?? "进程路径不可读"}
              </p>
              <p className="mt-2 flex items-center gap-1.5 text-muted-foreground">
                <Clock3 className="size-3.5" />
                启动于 {formatStartedAt(selectedProcess.startedAt)}
              </p>
            </div>
          )}
          {terminateError && (
            <div className="flex gap-2 rounded-md border border-destructive/30 bg-destructive/5 px-3 py-2 text-xs text-destructive">
              <AlertTriangle className="mt-0.5 size-3.5 shrink-0" />
              <span>{terminateError}</span>
            </div>
          )}
          <DialogFooter>
            <Button
              variant="outline"
              onClick={closeTerminateDialog}
              disabled={terminating}
            >
              取消
            </Button>
            <Button
              variant="destructive"
              onClick={() => void terminateSelected()}
              disabled={terminating}
            >
              {terminating && <Loader2 className="size-4 animate-spin" />}
              {terminating ? "正在终止" : "确认终止"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

export default FileOccupancyPage;

