import { Fragment, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  Activity,
  ChevronDown,
  ChevronRight,
  Eraser,
  Loader2,
  Network,
  Radar,
  RefreshCw,
  Server,
  Wifi,
  XCircle,
} from "lucide-react";
import { api } from "@/lib/api";
import {
  EVT_NET_LOG,
  type LocalNetInfo,
  type NetLogEvent,
  type PingResult,
  type ProbeResult,
} from "@/lib/net-types";
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
import { Label } from "@/components/ui/label";
import NetLogView from "@/components/net/NetLogView";

function ProbeResultView({ result }: { result: ProbeResult }) {
  const ok = result.success;
  return (
    <div
      className={cn(
        "flex flex-wrap items-center gap-x-4 gap-y-1 rounded-md border p-2 text-xs",
        ok
          ? "border-emerald-500/40 bg-emerald-500/5"
          : "border-destructive/40 bg-destructive/5"
      )}
    >
      <Badge variant={ok ? "secondary" : "destructive"} className="gap-1">
        {ok ? <Activity className="size-3" /> : <XCircle className="size-3" />}
        {ok ? "连接成功" : "连接失败"}
      </Badge>
      <span className="font-mono">
        {result.host}:{result.port}
      </span>
      <span>耗时 {result.elapsedMs} ms</span>
      {ok ? (
        <span className="text-emerald-600">TCP 握手成功</span>
      ) : (
        <span className="text-destructive">{result.reason}</span>
      )}
      {result.resolvedIp && (
        <span className="text-muted-foreground">解析 IP {result.resolvedIp}</span>
      )}
      {result.sourceIp && (
        <span className="text-muted-foreground">出口 IP {result.sourceIp}</span>
      )}
      {result.route && (
        <span className="text-muted-foreground">
          路由 {result.route.dest} → 网关 {result.route.gateway}（
          {result.route.interface}）
        </span>
      )}
    </div>
  );
}

function PingResultView({ result }: { result: PingResult }) {
  const ok = result.success;
  return (
    <div
      className={cn(
        "flex flex-wrap items-center gap-x-4 gap-y-1 rounded-md border p-2 text-xs",
        ok
          ? "border-emerald-500/40 bg-emerald-500/5"
          : "border-destructive/40 bg-destructive/5"
      )}
    >
      <Badge variant={ok ? "secondary" : "destructive"} className="gap-1">
        {ok ? <Activity className="size-3" /> : <XCircle className="size-3" />}
        {ok ? "Ping 成功" : "Ping 失败"}
      </Badge>
      <span>
        {result.received}/{result.sent} 成功
      </span>
      <span>丢包率 {result.lossPercent.toFixed(0)}%</span>
      {result.avgMs != null && <span>平均 RTT {result.avgMs.toFixed(1)} ms</span>}
      {result.minMs != null && <span>最小 {result.minMs.toFixed(1)} ms</span>}
      {result.maxMs != null && <span>最大 {result.maxMs.toFixed(1)} ms</span>}
      {!ok && result.reason && <span className="text-destructive">{result.reason}</span>}
      {result.resolvedIp && (
        <span className="text-muted-foreground">目标 {result.resolvedIp}</span>
      )}
    </div>
  );
}

export default function NetworkToolsPage() {
  const [info, setInfo] = useState<LocalNetInfo | null>(null);
  const [logs, setLogs] = useState<NetLogEvent[]>([]);
  const [infoOpen, setInfoOpen] = useState(false); // 本机网络信息默认折叠

  // TCP 检测
  const [host, setHost] = useState("");
  const [port, setPort] = useState("7788");
  const [timeoutMs, setTimeoutMs] = useState("3000");
  const [probing, setProbing] = useState(false);
  const [probe, setProbe] = useState<ProbeResult | null>(null);

  // Ping
  const [pingCount, setPingCount] = useState("4");
  const [pinging, setPinging] = useState(false);
  const [ping, setPing] = useState<PingResult | null>(null);

  const pushLog = (level: NetLogEvent["level"], message: string) =>
    setLogs((l) => [...l, { level, message, time: Date.now() / 1000 }]);

  const refreshInfo = async () => {
    try {
      setInfo(await api.netLocalInfo());
    } catch (e) {
      pushLog("error", `获取本机网络信息失败：${e}`);
    }
  };

  useEffect(() => {
    refreshInfo();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    const un = listen<NetLogEvent>(EVT_NET_LOG, (e) => {
      setLogs((l) => [...l, e.payload]);
    });
    return () => {
      un.then((f) => f());
    };
  }, []);

  const runProbe = async () => {
    if (probing) return;
    const p = parseInt(port, 10);
    const t = parseInt(timeoutMs, 10);
    if (!host.trim()) {
      pushLog("error", "请输入目标 IP 或主机名");
      return;
    }
    if (Number.isNaN(p) || p < 1 || p > 65535) {
      pushLog("error", "端口必须在 1-65535 之间");
      return;
    }
    if (Number.isNaN(t) || t < 100) {
      pushLog("error", "超时时间必须 ≥ 100 ms");
      return;
    }
    setProbing(true);
    setProbe(null);
    try {
      const r = await api.netTcpProbe(host.trim(), p, t);
      setProbe(r);
    } catch (e) {
      pushLog("error", `检测命令执行失败：${e}`);
    } finally {
      setProbing(false);
    }
  };

  const runPing = async () => {
    if (pinging) return;
    const c = parseInt(pingCount, 10);
    const t = parseInt(timeoutMs, 10);
    if (!host.trim()) {
      pushLog("error", "请输入目标 IP 或主机名");
      return;
    }
    if (Number.isNaN(c) || c < 1 || c > 10) {
      pushLog("error", "Ping 次数必须在 1-10 之间");
      return;
    }
    if (Number.isNaN(t) || t < 100) {
      pushLog("error", "超时时间必须 ≥ 100 ms");
      return;
    }
    setPinging(true);
    setPing(null);
    try {
      const r = await api.netPing(host.trim(), c, t);
      setPing(r);
    } catch (e) {
      pushLog("error", `Ping 命令执行失败：${e}`);
    } finally {
      setPinging(false);
    }
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Enter") runProbe();
  };

  return (
    <div className="flex h-full flex-col gap-3 p-3">
      {/* ---- 本机网络信息（默认折叠） ---- */}
      <Card className="gap-2">
        <Collapsible open={infoOpen} onOpenChange={setInfoOpen}>
          <CardHeader className="px-4 py-2.5">
            <div className="flex items-center gap-2">
              <CollapsibleTrigger asChild>
                <button
                  className="flex flex-1 items-center gap-2 rounded-md text-left hover:bg-muted/40"
                  title={infoOpen ? "收起本机网络信息" : "展开本机网络信息"}
                >
                  {infoOpen ? (
                    <ChevronDown className="size-4 shrink-0 text-muted-foreground" />
                  ) : (
                    <ChevronRight className="size-4 shrink-0 text-muted-foreground" />
                  )}
                  <Wifi className="size-4 shrink-0 text-primary" />
                  <CardTitle className="text-sm">本机网络信息（IPv4）</CardTitle>
                </button>
              </CollapsibleTrigger>
              <Button
                variant="ghost"
                size="icon"
                className="size-7"
                onClick={refreshInfo}
                title="刷新"
              >
                <RefreshCw className="size-3.5" />
              </Button>
            </div>
          </CardHeader>
          <CollapsibleContent>
            <CardContent className="space-y-2 px-4 pb-3 text-xs">
          <div className="flex flex-wrap gap-x-5 gap-y-1">
            <span>
              主机名：<b>{info?.hostname ?? "--"}</b>
            </span>
            <span>
              系统：<b>{info?.os ?? "--"}</b>
            </span>
          </div>

          {info?.interfaces.map((iface) => (
            <div key={iface.name} className="rounded-md border bg-muted/20 p-2">
              <div className="flex items-center gap-2">
                <Network className="size-3.5 text-muted-foreground" />
                <span className="font-medium">{iface.name}</span>
                {iface.mac && (
                  <span className="font-mono text-muted-foreground">{iface.mac}</span>
                )}
              </div>
              <div className="mt-1 flex flex-wrap gap-x-4 gap-y-0.5 font-mono text-[11px] text-muted-foreground">
                {iface.ips.map((ip) => (
                  <span key={ip.addr}>
                    IP {ip.addr}/{ip.prefixLen}（掩码 {ip.netmask}）
                  </span>
                ))}
                {iface.gateways.length > 0 && (
                  <span>网关 {iface.gateways.join(", ")}</span>
                )}
                {iface.dns.length > 0 && <span>DNS {iface.dns.join(", ")}</span>}
              </div>
            </div>
          ))}
          {info && info.interfaces.length === 0 && (
            <div className="text-muted-foreground">未获取到网卡信息</div>
          )}

          {info && info.routes.length > 0 && (
            <div className="rounded-md border bg-muted/20 p-2">
              <div className="mb-1 font-medium">路由表（IPv4）</div>
              <div className="grid grid-cols-[1fr_1fr_1.2fr_auto] gap-x-3 gap-y-0.5 font-mono text-[11px] text-muted-foreground">
                <span className="text-[10px] uppercase text-muted-foreground/60">目标</span>
                <span className="text-[10px] uppercase text-muted-foreground/60">网关</span>
                <span className="text-[10px] uppercase text-muted-foreground/60">接口</span>
                <span className="text-[10px] uppercase text-muted-foreground/60">Metric</span>
                {info.routes.map((r, i) => (
                  <Fragment key={i}>
                    <span>{r.dest}</span>
                    <span>{r.gateway}</span>
                    <span className="truncate">{r.interface}</span>
                    <span>{r.metric}</span>
                  </Fragment>
                ))}
              </div>
            </div>
          )}
            </CardContent>
          </CollapsibleContent>
        </Collapsible>
      </Card>

      {/* ---- 端口检测 + Ping ---- */}
      <Card className="gap-2">
        <CardHeader className="px-4 py-2.5">
          <CardTitle className="flex items-center gap-2 text-sm">
            <Server className="size-4 text-primary" />
            端口检测（TCP）与 Ping
          </CardTitle>
        </CardHeader>
        <CardContent className="space-y-3 px-4 pb-3">
          <div className="flex flex-wrap items-end gap-2">
            <div className="w-48">
              <Label className="text-[11px]">目标 IP / 主机名</Label>
              <Input
                value={host}
                onChange={(e) => setHost(e.target.value)}
                onKeyDown={onKeyDown}
                placeholder="192.168.1.10"
                className="h-8 text-sm"
              />
            </div>
            <div className="w-24">
              <Label className="text-[11px]">端口</Label>
              <Input
                value={port}
                onChange={(e) => setPort(e.target.value)}
                placeholder="7788"
                className="h-8 text-sm"
              />
            </div>
            <div className="w-28">
              <Label className="text-[11px]">超时（ms）</Label>
              <Input
                value={timeoutMs}
                onChange={(e) => setTimeoutMs(e.target.value)}
                placeholder="3000"
                className="h-8 text-sm"
              />
            </div>
            <Button size="sm" className="h-8" onClick={runProbe} disabled={probing}>
              {probing ? <Loader2 className="animate-spin" /> : <Radar className="size-3.5" />}
              {probing ? "检测中…" : "检测端口"}
            </Button>
            <div className="w-24">
              <Label className="text-[11px]">Ping 次数</Label>
              <Input
                value={pingCount}
                onChange={(e) => setPingCount(e.target.value)}
                placeholder="4"
                className="h-8 text-sm"
              />
            </div>
            <Button size="sm" variant="outline" className="h-8" onClick={runPing} disabled={pinging}>
              {pinging ? <Loader2 className="animate-spin" /> : <Activity className="size-3.5" />}
              {pinging ? "Ping 中…" : "Ping"}
            </Button>
            <Button
              size="sm"
              variant="ghost"
              className="ml-auto h-8"
              onClick={() => setLogs([])}
              title="清空日志"
            >
              <Eraser className="size-3.5" />
              清空日志
            </Button>
          </div>

          {probe && <ProbeResultView result={probe} />}
          {ping && <PingResultView result={ping} />}
        </CardContent>
      </Card>

      {/* ---- 日志框（下方） ---- */}
      <div className="min-h-0 flex-1 rounded-lg border bg-muted/20">
        <NetLogView logs={logs} emptyText="暂无日志，点击「检测端口」或「Ping」开始" />
      </div>
    </div>
  );
}
