import { useEffect, useMemo, useState } from "react";
import {
  ChevronDown,
  ChevronUp,
  FolderOpen,
  History,
  Loader2,
  Network,
  Play,
  RefreshCw,
  Square,
  Trash2,
  Zap,
} from "lucide-react";
import { api, pickFolder } from "@/lib/api";
import type { RecentConnection, RemoteInfo, SyncOptions } from "@/lib/sync-types";
import { formatBytes } from "@/lib/utils";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Progress } from "@/components/ui/progress";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Separator } from "@/components/ui/separator";
import { Slider } from "@/components/ui/slider";
import { Switch } from "@/components/ui/switch";

interface Props {
  running: boolean;
  syncing: boolean; // scanning/connecting phase
  stopping: boolean;
  collapsed: boolean;
  onToggleCollapsed: () => void;
  onStart: (opts: SyncOptions) => Promise<void>;
  onStop: () => void;
}

export default function ClientPanel({
  running,
  syncing,
  stopping,
  collapsed,
  onToggleCollapsed,
  onStart,
  onStop,
}: Props) {
  const [ip, setIp] = useState("");
  const [port, setPort] = useState("7788");
  const [shares, setShares] = useState<string[]>([]);
  const [share, setShare] = useState("");
  const [localDir, setLocalDir] = useState("");
  const [threads, setThreads] = useState(4);
  const [bandwidth, setBandwidth] = useState(0); // MB/s, 0 = unlimited
  const [incremental, setIncremental] = useState(true);
  const [rescanOnInterrupt, setRescanOnInterrupt] = useState(false);
  const [deleteRemoved, setDeleteRemoved] = useState(false);
  const [connected, setConnected] = useState(false);
  const [remoteInfo, setRemoteInfo] = useState<RemoteInfo | null>(null);
  const [loadingInfo, setLoadingInfo] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [recent, setRecent] = useState<RecentConnection[]>([]);

  const refreshRecent = async () => {
    try {
      setRecent(await api.listRecentConnections());
    } catch {
      /* ignore */
    }
  };

  useEffect(() => {
    refreshRecent();
  }, []);

  const parsePort = () => {
    const p = parseInt(port, 10);
    if (Number.isNaN(p) || p < 1 || p > 65535) throw new Error("端口必须在 1-65535 之间");
    return p;
  };

  const connect = async () => {
    setBusy(true);
    setError(null);
    try {
      const p = parsePort();
      if (!ip.trim()) throw new Error("请输入节点 A 的 IP 地址");
      const list = await api.clientListShares(ip.trim(), p);
      setShares(list);
      setShare(list[0] ?? "");
      setConnected(true);
      if (list.length > 0) {
        await api.saveRecentConnection(ip.trim(), p, list[0], localDir);
        await refreshRecent();
        await loadRemoteInfo(ip.trim(), p, list[0]);
      }
    } catch (e) {
      setConnected(false);
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const loadRemoteInfo = async (i: string, p: number, s: string) => {
    setLoadingInfo(true);
    setRemoteInfo(null);
    try {
      const info = await api.clientRemoteInfo(i, p, s);
      setRemoteInfo(info);
    } catch (e) {
      setError(`获取远程目录信息失败: ${e}`);
    } finally {
      setLoadingInfo(false);
    }
  };

  const onShareChange = async (s: string) => {
    setShare(s);
    if (!connected) return;
    try {
      await api.saveRecentConnection(ip.trim(), parsePort(), s, localDir);
      await refreshRecent();
      await loadRemoteInfo(ip.trim(), parsePort(), s);
    } catch (e) {
      setError(String(e));
    }
  };

  const applyRecent = async (id: string) => {
    const rc = recent.find((r) => String(r.id) === id);
    if (!rc) return;
    setIp(rc.ip);
    setPort(String(rc.port));
    setShare(rc.share);
    setLocalDir(rc.localDir);
    setConnected(true);
    try {
      const list = await api.clientListShares(rc.ip, rc.port);
      setShares(list);
      await loadRemoteInfo(rc.ip, rc.port, rc.share);
    } catch (e) {
      setError(String(e));
    }
  };

  /** Effective sync target: local target folder + a subfolder named after the remote share. */
  const syncTargetDir = useMemo(() => {
    const name = share.split(/[\\/]/).filter(Boolean).pop();
    if (!localDir || !name) return "";
    return `${localDir.replace(/[\\/]+$/, "")}\\${name}`;
  }, [share, localDir]);

  const startSync = async () => {
    setError(null);
    try {
      const p = parsePort();
      if (!ip.trim()) throw new Error("请输入节点 A 的 IP 地址");
      if (!share) throw new Error("请先连接节点 A 并选择一个共享文件夹");
      if (!localDir.trim()) throw new Error("请选择本机目标文件夹");
      await api.saveRecentConnection(ip.trim(), p, share, localDir).catch(() => {});
      await refreshRecent();
      await onStart({
        remoteIp: ip.trim(),
        remotePort: p,
        share,
        localDir: syncTargetDir || localDir,
        threads,
        bandwidthMbps: bandwidth,
        incremental,
        rescanOnInterrupt,
        deleteRemoved,
      });
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <Card className="gap-3 py-4">
      <CardHeader className="flex-row items-center justify-between px-4 py-0">
        <CardTitle className="flex items-center gap-2 text-sm">
          <Network className="size-4 text-primary" />
          节点 B · 客户端（同步到本机）
        </CardTitle>
        <div className="flex items-center gap-1">
          <Badge variant={connected ? "default" : "secondary"}>
            {connected ? "已连接" : "未连接"}
          </Badge>
          <Button
            variant="ghost"
            size="icon"
            className="size-6"
            onClick={onToggleCollapsed}
            title={collapsed ? "展开" : "收起"}
          >
            {collapsed ? <ChevronDown /> : <ChevronUp />}
          </Button>
        </div>
      </CardHeader>

      {!collapsed && (
        <CardContent className="flex min-h-0 flex-1 flex-col space-y-3 px-4">
        {/* Connection */}
        <div className="grid grid-cols-[1fr_120px_auto] gap-2">
          <div className="space-y-1">
            <Label htmlFor="cli-ip" className="text-xs">节点 A IP</Label>
            <Input id="cli-ip" value={ip} onChange={(e) => setIp(e.target.value)} placeholder="192.168.1.100" />
          </div>
          <div className="space-y-1">
            <Label htmlFor="cli-port" className="text-xs">端口</Label>
            <Input id="cli-port" value={port} onChange={(e) => setPort(e.target.value)} placeholder="7788" />
          </div>
          <div className="flex items-end">
            <Button variant="outline" onClick={connect} disabled={busy} className="gap-1.5">
              {busy ? <Loader2 className="size-4 animate-spin" /> : <RefreshCw className="size-4" />}
              连接
            </Button>
          </div>
        </div>

        {/* Share + local folder */}
        <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
          <div className="space-y-1">
            <Label className="text-xs">远程共享文件夹</Label>
            <Select value={share} onValueChange={onShareChange} disabled={!connected}>
              <SelectTrigger className="w-full">
                <SelectValue placeholder="连接后选择" />
              </SelectTrigger>
              <SelectContent>
                {shares.length === 0 && (
                  <div className="px-2 py-1.5 text-xs text-muted-foreground">暂无共享文件夹</div>
                )}
                {shares.map((s) => (
                  <SelectItem key={s} value={s} className="max-w-75">
                    <span className="truncate font-mono">{s}</span>
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            {loadingInfo && (
              <p className="flex items-center gap-1 text-[10px] text-muted-foreground">
                <Loader2 className="size-3 animate-spin" /> 正在扫描远程目录…
              </p>
            )}
            {remoteInfo && !loadingInfo && (
              <p className="text-[10px] text-muted-foreground">
                远程共 {remoteInfo.totalFiles.toLocaleString()} 个文件 ·{" "}
                {formatBytes(remoteInfo.totalBytes)}
              </p>
            )}
          </div>

          <div className="space-y-1">
            <Label className="text-xs">本机目标文件夹</Label>
            <div className="flex gap-2">
              <Input
                value={localDir}
                onChange={(e) => setLocalDir(e.target.value)}
                placeholder="选择本机文件夹"
                className="font-mono text-xs"
              />
              <Button
                variant="outline"
                size="icon"
                title="选择文件夹"
                onClick={async () => {
                  const f = await pickFolder();
                  if (f) setLocalDir(f);
                }}
              >
                <FolderOpen />
              </Button>
            </div>
            {syncTargetDir && (
              <p className="text-[10px] text-muted-foreground">
                将同步到：<span className="font-mono">{syncTargetDir}</span>
              </p>
            )}
          </div>
        </div>

        <Separator />

        {/* Sync options */}
        <div className="space-y-3">
          <div className="space-y-1.5">
            <div className="flex items-center justify-between text-xs">
              <Label>同步线程数</Label>
              <span className="font-mono text-muted-foreground">{threads}</span>
            </div>
            <Slider
              min={1}
              max={512}
              step={1}
              value={[threads]}
              onValueChange={(v) => setThreads(v[0])}
            />
            <p
              className={`text-[10px] ${
                threads > 64 ? "text-amber-500" : "text-muted-foreground"
              }`}
            >
              {threads > 64
                ? `警告：${threads} 线程适合海量小文件；若以几十 MB 以上的大文件为主，建议降到 16 以下，否则磁盘写回会成为瓶颈反而更慢`
                : threads > 32
                  ? `提示：${threads} 线程偏高，大文件为主时 8~16 线程更优`
                  : "大量小文件可调高线程数；大文件为主时 8~16 线程更优"}
            </p>
          </div>

          <div className="space-y-1.5">
            <div className="flex items-center justify-between text-xs">
              <Label>最大总带宽</Label>
              <span className="font-mono text-muted-foreground">
                {bandwidth === 0 ? "不限速" : `${bandwidth} MB/s`}
              </span>
            </div>
            <Slider
              min={0}
              max={1024}
              step={1}
              value={[bandwidth]}
              onValueChange={(v) => setBandwidth(v[0])}
            />
            <p className="text-[10px] text-muted-foreground">0 表示不限速（适用于几百 TB 的大目录）</p>
          </div>

          <div className="flex items-center justify-between">
            <div className="flex items-center gap-1.5 text-xs">
              <Zap className="size-3.5" />
              <Label>增量同步（跳过相同文件）</Label>
            </div>
            <Switch checked={incremental} onCheckedChange={setIncremental} />
          </div>

          <label
            htmlFor="rescan-on-interrupt"
            className="flex cursor-pointer items-start gap-2 rounded-md border border-border/70 px-2.5 py-2"
          >
            <input
              id="rescan-on-interrupt"
              type="checkbox"
              checked={rescanOnInterrupt}
              onChange={(event) => setRescanOnInterrupt(event.target.checked)}
              className="mt-0.5 size-4 shrink-0 accent-primary"
            />
            <span className="min-w-0">
              <span className="flex items-center gap-1.5 text-xs font-medium">
                <RefreshCw className="size-3.5" />
                扫描中断后自动重新扫描
              </span>
              <span className="mt-0.5 block text-[10px] leading-4 text-muted-foreground">
                使用本机临时磁盘数据库记录已扫描路径；重扫时跳过重复路径，最多重试 30 次，任务结束后自动清理
              </span>
            </span>
          </label>

          <div className="flex items-center justify-between">
            <div className="flex items-center gap-1.5 text-xs">
              <Trash2 className="size-3.5" />
              <Label>同步已删除的文件/文件夹</Label>
            </div>
            <Switch checked={deleteRemoved} onCheckedChange={setDeleteRemoved} />
          </div>
        </div>

        {/* Recent connections */}
        <div className="space-y-1">
          <Label className="flex items-center gap-1 text-xs">
            <History className="size-3" /> 最近连接
          </Label>
          <Select onValueChange={applyRecent}>
            <SelectTrigger className="w-full">
              <SelectValue placeholder="选择历史连接" />
            </SelectTrigger>
            <SelectContent>
              {recent.length === 0 && (
                <div className="px-2 py-1.5 text-xs text-muted-foreground">暂无历史连接</div>
              )}
              {recent.map((r) => (
                <SelectItem key={r.id} value={String(r.id)}>
                  {r.ip}:{r.port} → {r.share.split(/[\\/]/).filter(Boolean).pop()}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>

        {error && <p className="text-xs text-destructive">{error}</p>}

        {syncing && (
          <div className="space-y-1">
            <p className="text-[10px] text-muted-foreground">正在准备同步…</p>
            <Progress value={null} className="h-1" />
          </div>
        )}

        <div className="mt-auto flex gap-2 pt-3">
          {running ? (
            <Button
              variant="destructive"
              className="flex-1"
              onClick={onStop}
              disabled={stopping}
            >
              <Square /> {stopping ? "正在停止…" : "停止同步"}
            </Button>
          ) : (
            <Button className="flex-1" onClick={startSync} disabled={busy || syncing}>
              <Play /> 开始同步
            </Button>
          )}
        </div>
        </CardContent>
      )}
    </Card>
  );
}
