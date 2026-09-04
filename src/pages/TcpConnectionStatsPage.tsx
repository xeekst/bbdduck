import { type FormEvent, useEffect, useMemo, useRef, useState } from "react";
import {
  Activity,
  AlertTriangle,
  ChevronLeft,
  ChevronRight,
  Loader2,
  Network,
  RefreshCw,
  Search,
} from "lucide-react";
import { api } from "@/lib/api";
import type {
  TcpConnectionDetail,
  TcpConnectionStatistics,
  TcpStateCount,
} from "@/lib/tcp-connection-types";
import { cn } from "@/lib/utils";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";

const PAGE_SIZE = 100;
const REFRESH_INTERVALS = [
  { value: "3000", label: "3 秒" },
  { value: "5000", label: "5 秒" },
  { value: "10000", label: "10 秒" },
  { value: "30000", label: "30 秒" },
  { value: "60000", label: "1 分钟" },
  { value: "300000", label: "5 分钟" },
];

const STATE_LABELS: Record<string, string> = {
  CLOSED: "已关闭",
  LISTENING: "监听",
  SYN_SENT: "SYN 已发送",
  SYN_RECEIVED: "SYN 已接收",
  ESTABLISHED: "已建立",
  FIN_WAIT_1: "等待 FIN ACK",
  FIN_WAIT_2: "等待对端 FIN",
  CLOSE_WAIT: "等待本地关闭",
  CLOSING: "双方关闭中",
  LAST_ACK: "等待最后 ACK",
  TIME_WAIT: "等待超时",
  DELETE_TCB: "内核清理",
};

interface DiagramNode {
  state: string;
  x: number;
  y: number;
}

const DIAGRAM_NODES: DiagramNode[] = [
  { state: "CLOSED", x: 485, y: 18 },
  { state: "LISTENING", x: 90, y: 118 },
  { state: "SYN_SENT", x: 350, y: 118 },
  { state: "SYN_RECEIVED", x: 650, y: 118 },
  { state: "ESTABLISHED", x: 500, y: 228 },
  { state: "FIN_WAIT_1", x: 770, y: 228 },
  { state: "FIN_WAIT_2", x: 925, y: 338 },
  { state: "CLOSE_WAIT", x: 245, y: 338 },
  { state: "LAST_ACK", x: 245, y: 458 },
  { state: "CLOSING", x: 500, y: 458 },
  { state: "TIME_WAIT", x: 770, y: 458 },
  { state: "DELETE_TCB", x: 20, y: 458 },
];

const DIAGRAM_EDGES = [
  { d: "M485 47 C390 55 275 95 240 118", label: "被动打开", x: 330, y: 78 },
  { d: "M560 76 L465 118", label: "主动打开 / send SYN", x: 495, y: 99 },
  { d: "M240 147 L650 147", label: "recv SYN / send SYN+ACK", x: 445, y: 137 },
  { d: "M500 147 L650 147", label: "同时打开", x: 575, y: 166 },
  { d: "M425 176 C440 205 485 214 535 228", label: "recv SYN+ACK", x: 440, y: 211 },
  { d: "M725 176 C710 205 665 216 635 228", label: "recv ACK", x: 700, y: 211 },
  { d: "M500 257 C410 270 350 305 320 338", label: "recv FIN", x: 390, y: 302 },
  { d: "M650 257 L770 257", label: "close / send FIN", x: 710, y: 246 },
  { d: "M845 286 C900 298 955 310 980 338", label: "recv ACK", x: 925, y: 304 },
  { d: "M980 396 C955 425 900 444 845 458", label: "recv FIN", x: 925, y: 430 },
  { d: "M770 286 C685 320 625 392 595 458", label: "同时关闭", x: 655, y: 373 },
  { d: "M650 487 L770 487", label: "recv ACK", x: 710, y: 476 },
  { d: "M320 396 L320 458", label: "close / send FIN", x: 382, y: 432 },
  { d: "M245 487 C135 475 95 300 105 176", label: "recv ACK → CLOSED", x: 105, y: 326 },
  { d: "M845 487 C1080 470 1080 95 635 47", label: "2MSL 超时 → CLOSED", x: 975, y: 175 },
  { d: "M170 487 L245 487", label: "清理完成", x: 207, y: 476 },
];

function errorText(error: unknown) {
  return typeof error === "string"
    ? error
    : error instanceof Error
      ? error.message
      : "查询失败，请重试";
}

function formatTime(seconds: number | null) {
  if (!seconds) return "—";
  return new Intl.DateTimeFormat("zh-CN", {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  }).format(new Date(seconds * 1000));
}

function formatBytes(bytes: number) {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB", "PB"];
  const index = Math.min(
    Math.floor(Math.log(bytes) / Math.log(1024)),
    units.length - 1
  );
  const value = bytes / 1024 ** index;
  return `${value.toLocaleString("zh-CN", {
    maximumFractionDigits: value >= 100 ? 0 : value >= 10 ? 1 : 2,
  })} ${units[index]}`;
}

function address(ip: string | null, port: number | null, family: string) {
  if (!ip || !port) return "—";
  return family === "IPv6" ? `[${ip}]:${port}` : `${ip}:${port}`;
}

function TcpStateDiagram({ counts }: { counts: TcpStateCount[] }) {
  const countByState = useMemo(
    () => new Map(counts.map((item) => [item.state, item.count])),
    [counts]
  );

  return (
    <div className="overflow-x-auto">
      <svg
        viewBox="0 0 1100 540"
        className="min-w-[820px]"
        role="img"
        aria-labelledby="tcp-state-title tcp-state-description"
      >
        <title id="tcp-state-title">TCP 状态转换与当前连接数量</title>
        <desc id="tcp-state-description">
          TCP 从关闭、握手、已建立到四次挥手及超时清理的主要状态转换，每个节点显示当前匹配连接数量。
        </desc>
        <defs>
          <marker
            id="tcp-arrow"
            viewBox="0 0 10 10"
            refX="9"
            refY="5"
            markerWidth="6"
            markerHeight="6"
            orient="auto-start-reverse"
          >
            <path d="M 0 0 L 10 5 L 0 10 z" className="fill-muted-foreground" />
          </marker>
        </defs>

        <g className="fill-none stroke-muted-foreground/60" strokeWidth="1.5">
          {DIAGRAM_EDGES.map((edge) => (
            <path key={edge.d} d={edge.d} markerEnd="url(#tcp-arrow)" />
          ))}
        </g>
        <g className="fill-muted-foreground text-[10px]">
          {DIAGRAM_EDGES.map((edge) => (
            <text key={`${edge.d}-label`} x={edge.x} y={edge.y} textAnchor="middle">
              {edge.label}
            </text>
          ))}
        </g>

        {DIAGRAM_NODES.map((node) => {
          const count = countByState.get(node.state) ?? 0;
          const active = count > 0;
          return (
            <g key={node.state} transform={`translate(${node.x} ${node.y})`}>
              <rect
                width="150"
                height="58"
                rx="10"
                className={cn(
                  "stroke-2",
                  active ? "fill-primary/10 stroke-primary" : "fill-card stroke-border"
                )}
              />
              <text
                x="75"
                y="22"
                textAnchor="middle"
                className="fill-foreground text-[12px] font-semibold"
              >
                {node.state}
              </text>
              <text
                x="75"
                y="43"
                textAnchor="middle"
                className={cn(
                  "text-[11px]",
                  active ? "fill-primary font-semibold" : "fill-muted-foreground"
                )}
              >
                {STATE_LABELS[node.state]} · {count.toLocaleString("zh-CN")}
              </text>
            </g>
          );
        })}
      </svg>
    </div>
  );
}

function ConnectionRow({ connection }: { connection: TcpConnectionDetail }) {
  return (
    <tr className="border-b last:border-0 hover:bg-muted/30">
      <td className="whitespace-nowrap px-3 py-2">
        <Badge variant="outline" className="font-normal">
          {connection.addressFamily}
        </Badge>
      </td>
      <td className="whitespace-nowrap px-3 py-2">
        <Badge
          variant="outline"
          className={cn(
            "font-mono text-[10px]",
            connection.state === "ESTABLISHED" &&
              "border-emerald-500/30 bg-emerald-500/8 text-emerald-700",
            connection.state === "LISTENING" &&
              "border-primary/30 bg-primary/8 text-primary"
          )}
        >
          {connection.state}
        </Badge>
      </td>
      <td className="whitespace-nowrap px-3 py-2 font-mono">
        {address(connection.localIp, connection.localPort, connection.addressFamily)}
      </td>
      <td className="whitespace-nowrap px-3 py-2 font-mono">
        {address(connection.remoteIp, connection.remotePort, connection.addressFamily)}
      </td>
      <td className="px-3 py-2">
        <div className="flex items-center gap-1.5 whitespace-nowrap">
          <span className="font-medium">{connection.processName}</span>
          <Badge variant="secondary" className="font-mono text-[10px]">
            {connection.pid}
          </Badge>
        </div>
      </td>
      <td className="max-w-80 px-3 py-2">
        <p className="truncate font-mono text-[11px]" title={connection.processPath ?? undefined}>
          {connection.processPath ?? "不可读"}
        </p>
      </td>
      <td className="whitespace-nowrap px-3 py-2 font-mono text-[11px]">
        {connection.bytesSent == null || connection.bytesReceived == null ? (
          <span className="text-muted-foreground">不可用</span>
        ) : (
          <div className="space-y-0.5">
            <p>↑ {formatBytes(connection.bytesSent)}</p>
            <p>↓ {formatBytes(connection.bytesReceived)}</p>
          </div>
        )}
      </td>
      <td className="whitespace-nowrap px-3 py-2 text-muted-foreground">
        {formatTime(connection.processStartedAt)}
      </td>
    </tr>
  );
}

export default function TcpConnectionStatsPage() {
  const [port, setPort] = useState("");
  const [sourceIp, setSourceIp] = useState("");
  const [localIp, setLocalIp] = useState("");
  const [result, setResult] = useState<TcpConnectionStatistics | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [page, setPage] = useState(0);
  const [autoRefresh, setAutoRefresh] = useState(true);
  const [refreshIntervalMs, setRefreshIntervalMs] = useState("5000");
  const queryInFlightRef = useRef(false);

  const query = async (
    values = { port, sourceIp, localIp },
    resetPage = true
  ) => {
    if (queryInFlightRef.current) return;
    const parsedPort = Number(values.port.trim());
    if (
      !/^\d+$/.test(values.port.trim()) ||
      !Number.isInteger(parsedPort) ||
      parsedPort < 1 ||
      parsedPort > 65535
    ) {
      setError("请输入 1-65535 之间的端口号");
      return;
    }
    queryInFlightRef.current = true;
    setLoading(true);
    setError(null);
    try {
      const next = await api.tcpConnectionStats(
        parsedPort,
        values.sourceIp.trim() || null,
        values.localIp.trim() || null
      );
      setPort(String(parsedPort));
      setSourceIp(next.sourceIp ?? "");
      setLocalIp(next.localIp ?? "");
      setResult(next);
      if (resetPage) setPage(0);
    } catch (queryError) {
      setError(errorText(queryError));
    } finally {
      queryInFlightRef.current = false;
      setLoading(false);
    }
  };

  useEffect(() => {
    if (!autoRefresh || !result) return;
    const values = {
      port: String(result.port),
      sourceIp: result.sourceIp ?? "",
      localIp: result.localIp ?? "",
    };
    const timer = window.setInterval(() => {
      void query(values, false);
    }, Number(refreshIntervalMs));
    return () => window.clearInterval(timer);
    // 只按最后一次已提交的查询条件和刷新配置重建定时器。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    autoRefresh,
    refreshIntervalMs,
    result?.port,
    result?.sourceIp,
    result?.localIp,
  ]);

  const submit = (event: FormEvent) => {
    event.preventDefault();
    void query();
  };

  const pageCount = result
    ? Math.max(1, Math.ceil(result.connections.length / PAGE_SIZE))
    : 1;
  const pageRows = result?.connections.slice(
    page * PAGE_SIZE,
    (page + 1) * PAGE_SIZE
  ) ?? [];
  const established =
    result?.stateCounts.find((item) => item.state === "ESTABLISHED")?.count ?? 0;
  const totalTraffic = result
    ? result.totalBytesSent + result.totalBytesReceived
    : 0;
  const trafficUnavailable =
    !!result &&
    result.trafficAvailableConnections === 0 &&
    result.trafficUnavailableConnections > 0;

  return (
    <div className="h-full overflow-auto p-4">
      <div className="mx-auto flex max-w-[1600px] flex-col gap-4">
        <div className="flex items-center gap-2">
          <Activity className="size-5 text-primary" />
          <h1 className="text-lg font-semibold">TCP 连接统计</h1>
          <p className="text-xs text-muted-foreground">
            按端口和可选 IP 条件查看 TCP 状态分布与连接详情
          </p>
        </div>

        <Card className="gap-3 py-4">
          <CardContent className="px-4">
            <form className="grid gap-3 lg:grid-cols-[160px_1fr_1fr_auto] lg:items-end" onSubmit={submit}>
              <div>
                <Label htmlFor="tcp-stats-port" className="text-[11px]">端口</Label>
                <Input
                  id="tcp-stats-port"
                  value={port}
                  onChange={(event) => setPort(event.target.value)}
                  placeholder="如 7788"
                  inputMode="numeric"
                  disabled={loading}
                  autoFocus
                />
              </div>
              <div>
                <Label htmlFor="tcp-stats-source" className="text-[11px]">
                  来源 IP（连接进入的 IP，可选）
                </Label>
                <Input
                  id="tcp-stats-source"
                  value={sourceIp}
                  onChange={(event) => setSourceIp(event.target.value)}
                  placeholder="如 10.10.1.25"
                  disabled={loading}
                />
              </div>
              <div>
                <Label htmlFor="tcp-stats-local" className="text-[11px]">
                  本地 IP（被连接的 IP，可选）
                </Label>
                <Input
                  id="tcp-stats-local"
                  value={localIp}
                  onChange={(event) => setLocalIp(event.target.value)}
                  placeholder="如 10.10.1.2"
                  disabled={loading}
                />
              </div>
              <Button type="submit" disabled={loading || !port.trim()}>
                {loading ? <Loader2 className="size-4 animate-spin" /> : <Search className="size-4" />}
                {loading ? "正在统计" : "开始统计"}
              </Button>
            </form>
            <p className="mt-2 text-[11px] text-muted-foreground">
              来源 IP 对应 TCP 远端地址；本地 IP 对应当前计算机被连接的地址。留空表示不过滤。
            </p>
            {error && (
              <div role="alert" className="mt-3 flex items-start gap-2 rounded-md border border-destructive/30 bg-destructive/5 px-3 py-2 text-xs text-destructive">
                <AlertTriangle className="mt-0.5 size-3.5 shrink-0" />
                <span>{error}</span>
              </div>
            )}
          </CardContent>
        </Card>

        {result && (
          <>
            <div className="grid grid-cols-2 gap-2 lg:grid-cols-3 xl:grid-cols-6" aria-live="polite">
              <div className="rounded-lg border bg-card px-3 py-2.5 shadow-xs">
                <p className="text-[10px] text-muted-foreground">当前连接累计总流量</p>
                <p className="mt-0.5 text-sm font-semibold">
                  {trafficUnavailable ? "不可用" : formatBytes(totalTraffic)}
                </p>
                <p className="mt-0.5 truncate text-[10px] text-muted-foreground">
                  ↑ {formatBytes(result.totalBytesSent)} · ↓ {formatBytes(result.totalBytesReceived)}
                </p>
              </div>
              {[
                ["匹配连接", result.totalConnections.toLocaleString("zh-CN")],
                ["监听连接", result.listenerCount.toLocaleString("zh-CN")],
                ["已建立", established.toLocaleString("zh-CN")],
                ["相关进程", result.processCount.toLocaleString("zh-CN")],
                ["采集耗时", `${result.elapsedMs.toLocaleString("zh-CN")} ms`],
              ].map(([label, value]) => (
                <div key={label} className="rounded-lg border bg-card px-3 py-2.5 shadow-xs">
                  <p className="text-[10px] text-muted-foreground">{label}</p>
                  <p className="mt-0.5 text-sm font-semibold">{value}</p>
                </div>
              ))}
            </div>

            {(result.trafficPermissionDenied ||
              result.trafficNewlyEnabledConnections > 0 ||
              result.trafficUnavailableConnections > 0) && (
              <div
                className={cn(
                  "flex items-start gap-2 rounded-md border px-3 py-2 text-xs",
                  result.trafficPermissionDenied
                    ? "border-amber-500/30 bg-amber-500/5 text-amber-700"
                    : "bg-muted/30 text-muted-foreground"
                )}
              >
                <AlertTriangle className="mt-0.5 size-3.5 shrink-0" />
                <span>
                  {result.trafficPermissionDenied &&
                    "部分连接尚未启用 Windows EStats，当前权限无法开启；请以管理员身份运行以获得流量数据。 "}
                  {result.trafficNewlyEnabledConnections > 0 &&
                    `已为 ${result.trafficNewlyEnabledConnections} 个连接开始采集流量，后续自动刷新将显示自启用以来的累计值。 `}
                  {result.trafficUnavailableConnections > 0 &&
                    `${result.trafficUnavailableConnections} 个活动连接的流量暂时不可读。`}
                </span>
              </div>
            )}

            <Card className="gap-0 py-0">
              <CardHeader className="flex-row flex-wrap items-center gap-2 border-b px-4 py-3 !pb-3">
                <div className="min-w-0 flex-1">
                  <CardTitle className="text-sm">TCP 状态转换图</CardTitle>
                  <p className="mt-1 text-[11px] text-muted-foreground">
                    节点数字是本次快照中处于该状态的连接数量 · 采集于 {formatTime(result.capturedAt)}
                  </p>
                </div>
                <div className="flex flex-wrap items-center gap-2">
                  <label className="flex cursor-pointer items-center gap-2 text-xs">
                    <Switch
                      checked={autoRefresh}
                      onCheckedChange={setAutoRefresh}
                      aria-label="自动刷新"
                    />
                    自动刷新
                  </label>
                  <Select value={refreshIntervalMs} onValueChange={setRefreshIntervalMs}>
                    <SelectTrigger size="sm" className="w-24" aria-label="自动刷新周期">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {REFRESH_INTERVALS.map((interval) => (
                        <SelectItem key={interval.value} value={interval.value}>
                          {interval.label}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                  <Button
                    variant="ghost"
                    size="sm"
                    disabled={loading}
                    onClick={() =>
                      void query(
                        {
                          port: String(result.port),
                          sourceIp: result.sourceIp ?? "",
                          localIp: result.localIp ?? "",
                        },
                        false
                      )
                    }
                  >
                    <RefreshCw className={loading ? "size-3.5 animate-spin" : "size-3.5"} />
                    立即刷新
                  </Button>
                </div>
              </CardHeader>
              <CardContent className="px-3 py-2">
                <TcpStateDiagram counts={result.stateCounts} />
              </CardContent>
            </Card>

            <Card className="gap-0 overflow-hidden py-0">
              <CardHeader className="flex-row items-center border-b px-4 py-3 !pb-3">
                <div className="min-w-0 flex-1">
                  <CardTitle className="flex items-center gap-2 text-sm">
                    <Network className="size-4 text-primary" />
                    连接详情
                  </CardTitle>
                  <p className="mt-1 text-[11px] text-muted-foreground">
                    当前显示 {result.connections.length.toLocaleString("zh-CN")} 条明细，每页 {PAGE_SIZE} 条；流量为当前仍存在连接的累计应用数据字节
                  </p>
                </div>
                {result.connections.length > PAGE_SIZE && (
                  <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
                    <Button
                      variant="outline"
                      size="icon"
                      className="size-7"
                      disabled={page === 0}
                      onClick={() => setPage((value) => Math.max(0, value - 1))}
                      aria-label="上一页"
                    >
                      <ChevronLeft className="size-3.5" />
                    </Button>
                    <span className="min-w-16 text-center">
                      {page + 1} / {pageCount}
                    </span>
                    <Button
                      variant="outline"
                      size="icon"
                      className="size-7"
                      disabled={page + 1 >= pageCount}
                      onClick={() => setPage((value) => Math.min(pageCount - 1, value + 1))}
                      aria-label="下一页"
                    >
                      <ChevronRight className="size-3.5" />
                    </Button>
                  </div>
                )}
              </CardHeader>
              {result.detailsTruncated && (
                <div className="flex items-start gap-2 border-b bg-amber-500/5 px-4 py-2 text-xs text-amber-700">
                  <AlertTriangle className="mt-0.5 size-3.5 shrink-0" />
                  <span>匹配连接超过 5,000 条；状态统计包含全部连接，详情表仅保留前 5,000 条。</span>
                </div>
              )}
              <CardContent className="p-0">
                {pageRows.length === 0 ? (
                  <div className="flex min-h-36 flex-col items-center justify-center gap-2 text-muted-foreground">
                    <Network className="size-6" />
                    <p className="text-sm">没有符合条件的 TCP 连接</p>
                  </div>
                ) : (
                  <div className="max-h-[520px] overflow-auto">
                    <table className="w-full min-w-[1280px] text-left text-xs">
                      <thead className="sticky top-0 z-10 border-b bg-card text-[10px] text-muted-foreground">
                        <tr>
                          <th className="px-3 py-2 font-medium">协议族</th>
                          <th className="px-3 py-2 font-medium">TCP 状态</th>
                          <th className="px-3 py-2 font-medium">本地地址</th>
                          <th className="px-3 py-2 font-medium">来源 / 远端地址</th>
                          <th className="px-3 py-2 font-medium">进程 / PID</th>
                          <th className="px-3 py-2 font-medium">进程路径</th>
                          <th className="px-3 py-2 font-medium">发送 / 接收</th>
                          <th className="px-3 py-2 font-medium">进程启动时间</th>
                        </tr>
                      </thead>
                      <tbody>
                        {pageRows.map((connection, index) => (
                          <ConnectionRow
                            key={`${connection.addressFamily}-${connection.localIp}-${connection.remoteIp}-${connection.remotePort}-${connection.pid}-${index}`}
                            connection={connection}
                          />
                        ))}
                      </tbody>
                    </table>
                  </div>
                )}
              </CardContent>
            </Card>
          </>
        )}

        {!result && !loading && (
          <div className="flex min-h-72 flex-col items-center justify-center gap-3 rounded-lg border bg-card text-muted-foreground">
            <div className="flex size-14 items-center justify-center rounded-full bg-muted">
              <Activity className="size-6" />
            </div>
            <div className="text-center">
              <p className="text-sm font-medium text-foreground">输入端口开始统计</p>
              <p className="mt-1 text-xs">状态转换图和连接详情将显示在这里</p>
            </div>
          </div>
        )}

        {loading && !result && (
          <div className="flex min-h-72 flex-col items-center justify-center gap-3 text-muted-foreground">
            <Loader2 className="size-8 animate-spin text-primary" />
            <p className="text-sm font-medium text-foreground">正在读取 TCP 连接表</p>
          </div>
        )}
      </div>
    </div>
  );
}
