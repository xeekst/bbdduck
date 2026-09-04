import { type FormEvent, useState } from "react";
import {
  Activity,
  AlertTriangle,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  Clock3,
  FileText,
  Loader2,
  Network,
  Radar,
  RefreshCw,
  Search,
  Server,
  ShieldAlert,
} from "lucide-react";
import { api } from "@/lib/api";
import type {
  PortEndpoint,
  PortOccupancyScanResult,
  PortOccupyingProcess,
  ProcessTreeNode,
} from "@/lib/port-occupancy-types";
import { cn } from "@/lib/utils";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";

function errorText(error: unknown) {
  return typeof error === "string"
    ? error
    : error instanceof Error
      ? error.message
      : "检测失败，请重试";
}

function formatStartedAt(seconds: number | null) {
  if (!seconds) return "无法读取";
  return new Intl.DateTimeFormat("zh-CN", {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  }).format(new Date(seconds * 1000));
}

function endpointAddress(ip: string, port: number, family: string) {
  return family === "IPv6" ? `[${ip}]:${port}` : `${ip}:${port}`;
}

function TreeRow({ node, depth = 0 }: { node: ProcessTreeNode; depth?: number }) {
  return (
    <>
      <div
        className={cn(
          "flex min-w-max items-center gap-2 border-l py-1.5 pr-2 text-xs",
          node.isTarget && "bg-primary/6 font-medium"
        )}
        style={{ paddingLeft: `${depth * 18 + 10}px` }}
      >
        {node.children.length > 0 ? (
          <ChevronDown className="size-3 shrink-0 text-muted-foreground" />
        ) : (
          <span className="size-3 shrink-0" />
        )}
        <Server className={cn("size-3.5 shrink-0", node.isTarget && "text-primary")} />
        <span>{node.name}</span>
        <Badge variant={node.isTarget ? "default" : "secondary"} className="font-mono text-[10px]">
          PID {node.pid}
        </Badge>
        <span className="text-[10px] text-muted-foreground">
          PPID {node.parentPid} · {node.threadCount} 线程
        </span>
        {node.isTarget && <span className="text-[10px] text-primary">端口占用进程</span>}
      </div>
      {node.children.map((child) => (
        <TreeRow key={child.pid} node={child} depth={depth + 1} />
      ))}
    </>
  );
}

function EndpointRow({ endpoint }: { endpoint: PortEndpoint }) {
  return (
    <div className="grid gap-2 rounded-md border bg-background px-3 py-2 text-xs md:grid-cols-[110px_minmax(220px,1fr)_minmax(180px,1fr)_120px] md:items-center">
      <div className="flex items-center gap-1.5">
        <Badge variant={endpoint.protocol === "TCP" ? "default" : "secondary"}>
          {endpoint.protocol}
        </Badge>
        <Badge variant="outline" className="font-normal">
          {endpoint.addressFamily}
        </Badge>
      </div>
      <div className="min-w-0">
        <p className="text-[10px] text-muted-foreground">本地监听 / 绑定地址</p>
        <p className="truncate font-mono" title={endpointAddress(endpoint.localIp, endpoint.localPort, endpoint.addressFamily)}>
          {endpointAddress(endpoint.localIp, endpoint.localPort, endpoint.addressFamily)}
        </p>
        {endpoint.wildcard && (
          <p className="mt-0.5 text-[10px] text-amber-700">
            监听该协议族的所有本机 IP
          </p>
        )}
      </div>
      <div className="min-w-0">
        <p className="text-[10px] text-muted-foreground">远端地址</p>
        <p className="truncate font-mono">
          {endpoint.remoteIp && endpoint.remotePort
            ? endpointAddress(endpoint.remoteIp, endpoint.remotePort, endpoint.addressFamily)
            : "—"}
        </p>
      </div>
      <div>
        <p className="text-[10px] text-muted-foreground">状态</p>
        <Badge
          variant="outline"
          className={cn(
            "mt-0.5 font-mono text-[10px]",
            endpoint.listening && "border-emerald-500/30 bg-emerald-500/8 text-emerald-700"
          )}
        >
          {endpoint.state}
        </Badge>
      </div>
    </div>
  );
}

function ProcessCard({ process }: { process: PortOccupyingProcess }) {
  const [treeOpen, setTreeOpen] = useState(false);
  const critical = process.appType === "critical";

  return (
    <div className="overflow-hidden rounded-lg border bg-card shadow-xs">
      <div className="flex items-start gap-3 border-b p-4">
        <div className="flex size-10 shrink-0 items-center justify-center rounded-lg bg-primary/8">
          <Server className="size-4 text-primary" />
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
              {critical ? <ShieldAlert className="size-3" /> : <Activity className="size-3" />}
              {critical ? "Windows 关键进程" : "运行中"}
            </Badge>
          </div>
          <p className="mt-1 break-all font-mono text-[11px] leading-5 text-muted-foreground">
            {process.path ?? "进程路径不可读（可能需要管理员权限）"}
          </p>
        </div>
      </div>

      <div className="grid grid-cols-2 gap-px bg-border sm:grid-cols-5">
        {[
          ["进程 PID", String(process.pid)],
          ["父进程 PID", String(process.parentPid)],
          ["启动时间", formatStartedAt(process.startedAt)],
          ["会话 ID", String(process.sessionId)],
          ["线程数", String(process.threadCount)],
        ].map(([label, value]) => (
          <div key={label} className="min-w-0 bg-card px-3 py-2.5">
            <p className="text-[10px] text-muted-foreground">{label}</p>
            <p className="mt-0.5 truncate text-xs font-medium" title={value}>
              {value}
            </p>
          </div>
        ))}
      </div>

      <div className="space-y-2 border-t bg-muted/20 p-3">
        <div>
          <p className="mb-1 flex items-center gap-1.5 text-[11px] font-medium text-muted-foreground">
            <FileText className="size-3.5" />
            启动命令
          </p>
          <p className="max-h-20 overflow-auto break-all rounded-md border bg-background px-2.5 py-2 font-mono text-[11px] leading-5">
            {process.commandLine ?? "启动命令不可读（可能需要管理员权限）"}
          </p>
        </div>
        <div>
          <p className="mb-1 flex items-center gap-1.5 text-[11px] font-medium text-muted-foreground">
            <Network className="size-3.5" />
            端口端点（{process.endpoints.length}）
          </p>
          <div className="space-y-1.5">
            {process.endpoints.map((endpoint, index) => (
              <EndpointRow
                key={`${endpoint.protocol}-${endpoint.addressFamily}-${endpoint.localIp}-${endpoint.remoteIp}-${index}`}
                endpoint={endpoint}
              />
            ))}
          </div>
        </div>
      </div>

      <Collapsible open={treeOpen} onOpenChange={setTreeOpen}>
        <CollapsibleTrigger asChild>
          <button className="flex w-full items-center gap-2 border-t px-4 py-3 text-left text-xs font-medium hover:bg-muted/40">
            {treeOpen ? (
              <ChevronDown className="size-4 text-muted-foreground" />
            ) : (
              <ChevronRight className="size-4 text-muted-foreground" />
            )}
            <Network className="size-4 text-primary" />
            进程树
            <span className="font-normal text-muted-foreground">
              查看父进程链和由该进程启动的子进程
            </span>
          </button>
        </CollapsibleTrigger>
        <CollapsibleContent>
          <div className="border-t bg-muted/10 p-3">
            {process.parentChain.length > 0 && (
              <div className="mb-3 rounded-md border bg-background px-3 py-2">
                <p className="mb-1.5 text-[10px] font-medium text-muted-foreground">父进程链</p>
                <div className="flex flex-wrap items-center gap-1.5 text-xs">
                  {process.parentChain.map((parent, index) => (
                    <span key={parent.pid} className="contents">
                      {index > 0 && <ChevronRight className="size-3 text-muted-foreground" />}
                      <span>{parent.name}</span>
                      <Badge variant="secondary" className="font-mono text-[10px]">
                        {parent.pid}
                      </Badge>
                    </span>
                  ))}
                  <ChevronRight className="size-3 text-muted-foreground" />
                  <span className="font-medium text-primary">{process.name}</span>
                  <Badge className="font-mono text-[10px]">{process.pid}</Badge>
                </div>
              </div>
            )}
            <div className="max-h-72 overflow-auto rounded-md border bg-background">
              <TreeRow node={process.processTree} />
            </div>
            {process.treeTruncated && (
              <p className="mt-2 text-[10px] text-amber-700">
                进程树过大，仅显示前 300 个节点或最多 12 层。
              </p>
            )}
          </div>
        </CollapsibleContent>
      </Collapsible>
    </div>
  );
}

export default function PortOccupancyPage() {
  const [port, setPort] = useState("");
  const [result, setResult] = useState<PortOccupancyScanResult | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const scan = async (value = port) => {
    const normalized = value.trim();
    const parsed = Number(normalized);
    if (!/^\d+$/.test(normalized) || !Number.isInteger(parsed) || parsed < 1 || parsed > 65535) {
      setError("请输入 1-65535 之间的端口号");
      return;
    }
    setPort(String(parsed));
    setLoading(true);
    setError(null);
    try {
      setResult(await api.portOccupancyScan(parsed));
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

  return (
    <div className="flex h-full flex-col gap-4 overflow-hidden p-4">
      <div className="flex shrink-0 items-center gap-2">
        <Radar className="size-5 text-primary" />
        <h1 className="text-lg font-semibold">端口检测</h1>
        <p className="text-xs text-muted-foreground">
          查询本机 TCP / UDP 端口、监听 IP、占用进程及进程树
        </p>
      </div>

      <Card className="shrink-0 gap-3 py-4">
        <CardContent className="px-4">
          <form className="flex flex-col gap-2 sm:flex-row" onSubmit={submit}>
            <div className="relative min-w-0 flex-1">
              <Search className="absolute left-3 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
              <Input
                value={port}
                onChange={(event) => setPort(event.target.value)}
                placeholder="输入要检查的端口，如 7788"
                className="pl-9 text-sm"
                inputMode="numeric"
                disabled={loading}
                autoFocus
              />
            </div>
            <Button type="submit" disabled={loading || !port.trim()}>
              {loading ? <Loader2 className="size-4 animate-spin" /> : <Radar className="size-4" />}
              {loading ? "正在查询" : "检测端口"}
            </Button>
          </form>
          <p className="mt-2 text-[11px] text-muted-foreground">
            同时检查 IPv4 / IPv6 的 TCP 监听、TCP 活跃连接和 UDP 绑定；受保护进程的路径或启动命令可能需要管理员权限才能读取。
          </p>
          {error && (
            <div className="mt-3 flex items-start gap-2 rounded-md border border-destructive/30 bg-destructive/5 px-3 py-2 text-xs text-destructive">
              <AlertTriangle className="mt-0.5 size-3.5 shrink-0" />
              <span>{error}</span>
            </div>
          )}
        </CardContent>
      </Card>

      {result && (
        <div className="grid shrink-0 grid-cols-2 gap-2 lg:grid-cols-5">
          <div className="rounded-lg border bg-card px-3 py-2.5 shadow-xs">
            <p className="text-[10px] text-muted-foreground">端口状态</p>
            <p className={cn("mt-0.5 text-sm font-semibold", result.occupied ? "text-destructive" : "text-emerald-700")}>
              {result.occupied ? "已占用" : "未占用"}
            </p>
          </div>
          {[
            ["检查端口", String(result.port)],
            ["占用进程", String(result.processes.length)],
            ["监听 / 绑定端点", String(result.listenerCount)],
            ["查询耗时", `${result.elapsedMs.toLocaleString("zh-CN")} ms`],
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
            <CardTitle className="text-sm">本机端口占用结果</CardTitle>
            <p className="mt-1 text-[11px] text-muted-foreground">
              {result
                ? `端口 ${result.port} 共找到 ${result.endpointCount} 个端点，涉及 ${result.processes.length} 个进程`
                : "尚未输入要检查的端口"}
            </p>
          </div>
          {result && (
            <Button
              variant="ghost"
              size="sm"
              onClick={() => void scan(String(result.port))}
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
                <p className="text-sm font-medium text-foreground">正在读取系统端口表</p>
                <p className="mt-1 text-xs">正在关联 TCP / UDP 端点、进程详情和进程树</p>
              </div>
            </div>
          ) : !result ? (
            <div className="flex h-full flex-col items-center justify-center gap-3 text-muted-foreground">
              <div className="flex size-14 items-center justify-center rounded-full bg-muted">
                <Radar className="size-6" />
              </div>
              <div className="text-center">
                <p className="text-sm font-medium text-foreground">输入端口开始检测</p>
                <p className="mt-1 text-xs">例如输入 80、443 或 7788</p>
              </div>
            </div>
          ) : !result.occupied ? (
            <div className="flex h-full flex-col items-center justify-center gap-3 text-muted-foreground">
              <div className="flex size-14 items-center justify-center rounded-full bg-emerald-500/10">
                <CheckCircle2 className="size-7 text-emerald-600" />
              </div>
              <div className="text-center">
                <p className="text-sm font-medium text-foreground">端口 {result.port} 当前未被占用</p>
                <p className="mt-1 text-xs">没有发现该本地端口的 TCP 或 UDP 端点</p>
              </div>
            </div>
          ) : (
            <ScrollArea className="h-full">
              <div className="space-y-3 p-4">
                <div className="flex items-start gap-2 rounded-md border border-amber-500/30 bg-amber-500/5 px-3 py-2 text-xs text-amber-700">
                  <Clock3 className="mt-0.5 size-3.5 shrink-0" />
                  <span>
                    端口状态会随进程启动、退出和连接变化；需要实时确认时请点击右上角“刷新”。
                  </span>
                </div>
                {result.processes.map((process) => (
                  <ProcessCard key={process.pid} process={process} />
                ))}
              </div>
            </ScrollArea>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
