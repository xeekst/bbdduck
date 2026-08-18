import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  ChevronDown,
  ChevronUp,
  Folder,
  FolderPlus,
  HardDrive,
  Play,
  Save,
  Server,
  Square,
  Trash2,
  X,
} from "lucide-react";
import { api, pickFolders } from "@/lib/api";
import { EVT_SERVER, type ServerConfig, type ServerEvent, type ServerStatus } from "@/lib/sync-types";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Slider } from "@/components/ui/slider";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

function defaultStatus(): ServerStatus {
  return { running: false, addr: null, shares: [] };
}

interface ServerPanelProps {
  collapsed: boolean;
  onToggleCollapsed: () => void;
}

export default function ServerPanel({ collapsed, onToggleCollapsed }: ServerPanelProps) {
  const [ip, setIp] = useState("0.0.0.0");
  const [port, setPort] = useState("7788");
  const [folders, setFolders] = useState<string[]>([]);
  const [scanWorkers, setScanWorkers] = useState(0); // 0 = 自动
  const [status, setStatus] = useState<ServerStatus>(defaultStatus);
  const [saved, setSaved] = useState<ServerConfig[]>([]);
  const [selectedId, setSelectedId] = useState<string>("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refreshSaved = async () => {
    try {
      setSaved(await api.listServerConfigs());
    } catch {
      /* ignore */
    }
  };

  useEffect(() => {
    refreshSaved();
    const un = listen<ServerEvent>(EVT_SERVER, (e) => {
      setStatus({
        running: e.payload.running,
        addr: e.payload.addr ?? null,
        shares: e.payload.shares ?? [],
      });
      if (e.payload.message) setError(e.payload.message);
    });
    return () => {
      un.then((fn) => fn());
    };
  }, []);

  const parsePort = () => {
    const p = parseInt(port, 10);
    if (Number.isNaN(p) || p < 1 || p > 65535) throw new Error("端口必须在 1-65535 之间");
    return p;
  };

  const parentDir = (p: string): string => {
    const idx = Math.max(p.lastIndexOf("\\"), p.lastIndexOf("/"));
    return idx > 0 ? p.slice(0, idx) : p;
  };

  const addFolder = async () => {
    const last = folders[folders.length - 1];
    const defaultPath = last ? parentDir(last) : undefined;
    const picked = await pickFolders(defaultPath);
    if (picked.length === 0) return;
    setFolders((prev) => {
      const next = [...prev];
      for (const f of picked) {
        if (!next.includes(f)) next.push(f);
      }
      return next;
    });
  };

  const removeFolder = (i: number) =>
    setFolders((prev) => prev.filter((_, idx) => idx !== i));

  const start = async () => {
    setBusy(true);
    setError(null);
    try {
      const p = parsePort();
      if (folders.length === 0) throw new Error("请至少选择一个要共享的文件夹");
      const st = await api.serverStart(ip.trim() || "0.0.0.0", p, folders, scanWorkers);
      setStatus(st);
      // auto-save a config so it's easy to restore later
      const name = folders[0].split(/[\\/]/).filter(Boolean).pop() || "共享文件夹";
      await api.saveServerConfig(name, ip.trim() || "0.0.0.0", p, folders, scanWorkers);
      await refreshSaved();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const stop = async () => {
    setBusy(true);
    setError(null);
    try {
      await api.serverStop();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const applySaved = (id: string) => {
    const cfg = saved.find((c) => String(c.id) === id);
    if (!cfg) return;
    setIp(cfg.ip);
    setPort(String(cfg.port));
    setFolders(cfg.folders);
    setScanWorkers(cfg.scanWorkers ?? 0);
    setSelectedId(id);
  };

  const deleteSaved = async () => {
    const id = parseInt(selectedId, 10);
    if (!id) return;
    try {
      await api.deleteServerConfig(id);
      setSelectedId("");
      await refreshSaved();
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <Card className="gap-3 py-4">
      <CardHeader className="flex-row items-center justify-between px-4 py-0">
        <CardTitle className="flex items-center gap-2 text-sm">
          <Server className="size-4 text-primary" />
          节点 A · 服务端（共享文件夹）
        </CardTitle>
        <div className="flex items-center gap-1">
          <Badge variant={status.running ? "default" : "secondary"}>
            {status.running ? "监听中" : "未启动"}
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
        <div className="grid grid-cols-[1fr_120px] gap-2">
          <div className="space-y-1">
            <Label htmlFor="srv-ip" className="text-xs">监听 IP</Label>
            <Input id="srv-ip" value={ip} onChange={(e) => setIp(e.target.value)} placeholder="0.0.0.0" />
          </div>
          <div className="space-y-1">
            <Label htmlFor="srv-port" className="text-xs">端口</Label>
            <Input id="srv-port" value={port} onChange={(e) => setPort(e.target.value)} placeholder="7788" />
          </div>
        </div>

        <div className="space-y-1">
          <Label className="text-xs">共享文件夹（可多选）</Label>
          <div className="h-40 space-y-1 overflow-auto pr-1">
            {folders.length === 0 && (
              <p className="rounded border border-dashed px-2 py-2 text-center text-[11px] text-muted-foreground">
                尚未添加共享文件夹
              </p>
            )}
            {folders.map((f, i) => (
              <div key={i} className="flex items-center gap-1">
                <div
                  className="flex min-w-0 flex-1 items-center gap-1.5 rounded border bg-muted/30 px-2 py-1"
                  title={f}
                >
                  <Folder className="size-3.5 shrink-0 text-amber-500" />
                  <span className="truncate font-mono text-xs">{f}</span>
                </div>
                <Button
                  variant="ghost"
                  size="icon"
                  className="size-6 shrink-0"
                  title="移除"
                  onClick={() => removeFolder(i)}
                >
                  <X />
                </Button>
              </div>
            ))}
          </div>
          <Button variant="outline" size="sm" className="w-full gap-1.5" onClick={addFolder}>
            <FolderPlus /> 添加共享文件夹
          </Button>
        </div>

        <div className="space-y-1.5">
          <div className="flex items-center justify-between text-xs">
            <Label>扫描并发数</Label>
            <span className="font-mono text-muted-foreground">
              {scanWorkers === 0 ? "自动" : `${scanWorkers} 线程`}
            </span>
          </div>
          <Slider
            min={0}
            max={32}
            step={1}
            value={[scanWorkers]}
            onValueChange={(v) => setScanWorkers(v[0])}
          />
          <p className="text-[10px] text-muted-foreground">
            0 = 自动（按本机 CPU 一半）；扫描超大目录时可适当调高并发
          </p>
        </div>

        <div className="space-y-1">
          <Label className="text-xs">已保存配置</Label>
          <div className="flex gap-2">
            <Select value={selectedId} onValueChange={applySaved}>
              <SelectTrigger className="flex-1">
                <SelectValue placeholder="选择历史配置" />
              </SelectTrigger>
              <SelectContent>
                {saved.length === 0 && (
                  <div className="px-2 py-1.5 text-xs text-muted-foreground">暂无保存的配置</div>
                )}
                {saved.map((c) => (
                  <SelectItem key={c.id} value={String(c.id)}>
                    {c.name} · {c.ip}:{c.port}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <Button
              variant="outline"
              size="icon"
              title="保存当前配置"
              onClick={async () => {
                try {
                  const p = parsePort();
                  if (folders.length === 0) throw new Error("请先添加共享文件夹");
                  const name = folders[0].split(/[\\/]/).filter(Boolean).pop() || "共享文件夹";
                  await api.saveServerConfig(name, ip.trim() || "0.0.0.0", p, folders, scanWorkers);
                  await refreshSaved();
                } catch (e) {
                  setError(String(e));
                }
              }}
            >
              <Save />
            </Button>
            <Button variant="outline" size="icon" title="删除所选配置" onClick={deleteSaved}>
              <Trash2 />
            </Button>
          </div>
        </div>

        {error && <p className="text-xs text-destructive">{error}</p>}

        {status.running && (
          <div className="flex items-center gap-2 text-xs text-muted-foreground">
            <HardDrive className="size-3.5" />
            <span className="truncate">
              监听于 {status.addr} · 共享 {status.shares.join(", ")}
            </span>
          </div>
        )}

        <div className="mt-auto flex gap-2 pt-3">
          {status.running ? (
            <Button variant="destructive" className="flex-1" onClick={stop} disabled={busy}>
              <Square /> 停止监听
            </Button>
          ) : (
            <Button className="flex-1" onClick={start} disabled={busy}>
              <Play /> 开启监听
            </Button>
          )}
        </div>
        </CardContent>
      )}
    </Card>
  );
}
