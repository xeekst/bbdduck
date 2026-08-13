// SSH 端口转发页面：上工具栏 + 可滚动隧道表格（多隧道并存）。
// - 状态/日志通过 ssh-tunnel-state / ssh-tunnel-log 事件实时更新
// - 每 2 秒轮询一次列表刷新流量计数
// - 「添加隧道」弹窗支持本地/远程/动态三种类型与数据流向动画
// - 点击「日志」弹窗查看当前隧道实时日志

import { useCallback, useEffect, useMemo, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  ArrowLeftRight,
  Loader2,
  Pencil,
  Play,
  RefreshCw,
  ScrollText,
  Square,
  Trash2,
} from "lucide-react";
import { api } from "@/lib/api";
import {
  EVT_SSH_TUNNEL_LOG,
  EVT_SSH_TUNNEL_STATE,
  TUNNEL_STATE_LABEL,
  TUNNEL_TYPE_LABEL,
  type TunnelItem,
  type TunnelLogEntry,
  type TunnelLogEvent,
  type TunnelState,
  type TunnelStateEvent,
} from "@/lib/ssh-types";
import { cn, formatBytes, formatDuration, formatTime } from "@/lib/utils";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { MiniFlow } from "@/components/ssh/FlowDiagram";
import TunnelFormDialog from "@/components/ssh/TunnelFormDialog";
import TunnelLogDialog from "@/components/ssh/TunnelLogDialog";

// ---------------- 状态徽标 ----------------

const STATE_STYLE: Record<
  TunnelState,
  { badge: string; dot: string }
> = {
  running: {
    badge: "border-emerald-500/40 bg-emerald-500/10 text-emerald-600",
    dot: "bg-emerald-500 tunnel-pulse",
  },
  connecting: {
    badge: "border-sky-500/40 bg-sky-500/10 text-sky-600",
    dot: "bg-sky-500 tunnel-pulse",
  },
  stopping: {
    badge: "border-amber-500/40 bg-amber-500/10 text-amber-600",
    dot: "bg-amber-500",
  },
  stopped: {
    badge: "border-border bg-muted/40 text-muted-foreground",
    dot: "bg-muted-foreground/50",
  },
  error: {
    badge: "border-destructive/40 bg-destructive/10 text-destructive",
    dot: "bg-destructive",
  },
};

const TYPE_STYLE: Record<string, string> = {
  local: "border-sky-500/40 bg-sky-500/10 text-sky-600",
  remote: "border-violet-500/40 bg-violet-500/10 text-violet-600",
  dynamic: "border-emerald-500/40 bg-emerald-500/10 text-emerald-600",
};

function StatusCell({ item }: { item: TunnelItem }) {
  const st = STATE_STYLE[item.state] ?? STATE_STYLE.stopped;
  return (
    <div className="flex flex-col gap-1">
      <Badge variant="outline" className={cn("w-fit gap-1", st.badge)}>
        <span className={cn("size-1.5 rounded-full", st.dot)} />
        {TUNNEL_STATE_LABEL[item.state]}
      </Badge>
      {item.state === "running" && item.connectedAt && (
        <span className="text-[10px] text-muted-foreground">
          运行 {formatDuration(Date.now() / 1000 - item.connectedAt)}
        </span>
      )}
      {item.state === "error" && item.error && (
        <span
          className="max-w-40 truncate text-[10px] text-destructive"
          title={item.error}
        >
          {item.error}
        </span>
      )}
    </div>
  );
}

// ---------------- 页面 ----------------

export default function PortForwardPage() {
  const [tunnels, setTunnels] = useState<TunnelItem[]>([]);
  const [logs, setLogs] = useState<Record<number, TunnelLogEntry[]>>({});
  const [dialogOpen, setDialogOpen] = useState(false);
  const [editing, setEditing] = useState<TunnelItem | null>(null);
  const [logTunnel, setLogTunnel] = useState<TunnelItem | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<number | null>(null);
  const [busyIds, setBusyIds] = useState<Set<number>>(new Set());
  const [notice, setNotice] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setTunnels(await api.sshTunnelList());
    } catch (e) {
      setNotice(`刷新失败：${e}`);
    }
  }, []);

  useEffect(() => {
    refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 轮询刷新运行计数
  useEffect(() => {
    const t = setInterval(refresh, 2000);
    return () => clearInterval(t);
  }, [refresh]);

  // 实时事件
  useEffect(() => {
    const un1 = listen<TunnelLogEvent>(EVT_SSH_TUNNEL_LOG, (e) => {
      const { id, ...entry } = e.payload;
      setLogs((m) => ({ ...m, [id]: [...(m[id] ?? []), entry] }));
    });
    const un2 = listen<TunnelStateEvent>(EVT_SSH_TUNNEL_STATE, (e) => {
      const p = e.payload;
      setTunnels((ts) =>
        ts.map((t) =>
          t.id === p.id
            ? {
                ...t,
                state: p.state,
                error: p.error,
                listenAddr: p.listenAddr,
                connectedAt: p.connectedAt,
              }
            : t
        )
      );
    });
    return () => {
      un1.then((f) => f());
      un2.then((f) => f());
    };
  }, []);

  const setBusy = (id: number, b: boolean) =>
    setBusyIds((s) => {
      const n = new Set(s);
      if (b) n.add(id);
      else n.delete(id);
      return n;
    });

  const withNotice = async (fn: () => Promise<void>) => {
    try {
      await fn();
    } catch (e) {
      setNotice(String(e));
    }
  };

  const startTunnel = (id: number) =>
    withNotice(async () => {
      setBusy(id, true);
      try {
        await api.sshTunnelStart(id);
      } finally {
        setBusy(id, false);
      }
    });

  const stopTunnel = (id: number) =>
    withNotice(async () => {
      setBusy(id, true);
      try {
        await api.sshTunnelStop(id);
      } finally {
        setBusy(id, false);
      }
    });

  const deleteTunnel = (item: TunnelItem) =>
    withNotice(async () => {
      setBusy(item.id, true);
      try {
        await api.sshTunnelDelete(item.id);
        setTunnels((ts) => ts.filter((t) => t.id !== item.id));
      } finally {
        setBusy(item.id, false);
      }
    });

  const openLogs = async (item: TunnelItem) => {
    setLogTunnel(item);
    try {
      const dbLogs = await api.sshTunnelLogs(item.id);
      setLogs((m) => ({ ...m, [item.id]: dbLogs }));
    } catch (e) {
      setNotice(String(e));
    }
  };

  const clearLogs = async () => {
    if (!logTunnel) return;
    try {
      await api.sshTunnelClearLogs(logTunnel.id);
      setLogs((m) => ({ ...m, [logTunnel.id]: [] }));
    } catch (e) {
      setNotice(String(e));
    }
  };

  const startAll = () =>
    withNotice(async () => {
      const stopped = tunnels.filter((t) => t.state === "stopped" || t.state === "error");
      for (const t of stopped) {
        try {
          await api.sshTunnelStart(t.id);
        } catch (e) {
          setNotice(`启动「${t.name}」失败：${e}`);
        }
      }
    });

  const stopAll = () =>
    withNotice(async () => {
      for (const t of tunnels) {
        if (t.state === "running" || t.state === "connecting") {
          await api.sshTunnelStop(t.id);
        }
      }
    });

  const logLines = useMemo(
    () => (logTunnel ? logs[logTunnel.id] ?? [] : []),
    [logTunnel, logs]
  );

  const runningCount = tunnels.filter((t) => t.state === "running").length;

  return (
    <div className="flex h-full flex-col gap-3 p-3">
      {/* ---- 顶部工具栏 ---- */}
      <Card className="shrink-0 gap-2">
        <CardHeader className="px-4 py-2.5">
          <div className="flex flex-wrap items-center gap-2">
            <ArrowLeftRight className="size-4 shrink-0 text-primary" />
            <CardTitle className="flex-1 text-sm">
              SSH 端口转发
              <span className="ml-2 text-xs font-normal text-muted-foreground">
                {runningCount}/{tunnels.length} 运行中
              </span>
            </CardTitle>
            <Button
              size="sm"
              variant="outline"
              onClick={startAll}
              disabled={tunnels.length === 0}
            >
              <Play className="size-3.5" />
              全部启动
            </Button>
            <Button
              size="sm"
              variant="outline"
              onClick={stopAll}
              disabled={runningCount === 0}
            >
              <Square className="size-3.5" />
              全部停止
            </Button>
            <Button size="sm" variant="outline" onClick={refresh}>
              <RefreshCw className="size-3.5" />
              刷新
            </Button>
            <Button
              size="sm"
              onClick={() => {
                setEditing(null);
                setDialogOpen(true);
              }}
            >
              <ArrowLeftRight className="size-3.5" />
              添加隧道
            </Button>
          </div>
          {notice && (
            <div className="flex items-center justify-between rounded-md border border-amber-500/40 bg-amber-500/10 px-3 py-1.5 text-xs text-amber-700">
              {notice}
              <button
                className="ml-2 text-amber-700/70 hover:text-amber-700"
                onClick={() => setNotice(null)}
              >
                关闭
              </button>
            </div>
          )}
        </CardHeader>
      </Card>

      {/* ---- 隧道表格（可滚动） ---- */}
      <Card className="min-h-0 flex-1 overflow-hidden">
        <CardContent className="h-full p-0">
          {tunnels.length === 0 ? (
            <div className="flex h-full flex-col items-center justify-center gap-3 text-muted-foreground">
              <ArrowLeftRight className="size-10 opacity-40" />
              <p className="text-sm">暂无隧道</p>
              <p className="text-xs">
                点击右上角「添加隧道」，支持本地 / 远程 / 动态三种转发方式
              </p>
              <Button
                variant="outline"
                size="sm"
                onClick={() => {
                  setEditing(null);
                  setDialogOpen(true);
                }}
              >
                添加第一个隧道
              </Button>
            </div>
          ) : (
            <div className="h-full overflow-auto">
              <table className="w-full border-collapse text-xs">
                <thead className="sticky top-0 z-10 bg-muted/95 backdrop-blur">
                  <tr className="text-left text-muted-foreground">
                    <th className="px-3 py-2 font-medium">状态</th>
                    <th className="px-3 py-2 font-medium">隧道</th>
                    <th className="px-3 py-2 font-medium">数据流向</th>
                    <th className="px-3 py-2 font-medium">监听地址</th>
                    <th className="px-3 py-2 font-medium">目标</th>
                    <th className="px-3 py-2 font-medium">SSH 服务器</th>
                    <th className="px-3 py-2 text-right font-medium">连接</th>
                    <th className="px-3 py-2 text-right font-medium">流量 ↑ / ↓</th>
                    <th className="px-3 py-2 text-right font-medium">操作</th>
                  </tr>
                </thead>
                <tbody>
                  {tunnels.map((t) => {
                    const busy = busyIds.has(t.id);
                    const active =
                      t.state === "running" || t.state === "connecting";
                    return (
                      <tr
                        key={t.id}
                        className="border-t hover:bg-muted/30"
                      >
                        <td className="px-3 py-2.5 align-top">
                          <StatusCell item={t} />
                        </td>
                        <td className="px-3 py-2.5 align-top">
                          <div className="flex flex-col gap-1">
                            <span className="font-medium">{t.name}</span>
                            <div className="flex items-center gap-1">
                              <Badge
                                variant="outline"
                                className={cn("h-4 px-1 text-[10px]", TYPE_STYLE[t.tunnelType])}
                              >
                                {TUNNEL_TYPE_LABEL[t.tunnelType]}
                              </Badge>
                              {t.tunnelType !== "dynamic" && (
                                <Badge
                                  variant="outline"
                                  className="h-4 px-1 text-[10px] text-muted-foreground"
                                >
                                  {t.proto.toUpperCase()}
                                </Badge>
                              )}
                            </div>
                          </div>
                        </td>
                        <td className="px-3 py-2.5 align-top">
                          <MiniFlow tunnelType={t.tunnelType} />
                        </td>
                        <td className="px-3 py-2.5 align-top font-mono text-[11px]">
                          {t.listenAddr || `${t.listenHost}:${t.listenPort}`}
                        </td>
                        <td className="px-3 py-2.5 align-top font-mono text-[11px]">
                          {t.tunnelType === "dynamic"
                            ? "任意（SOCKS5）"
                            : `${t.targetHost}:${t.targetPort}`}
                        </td>
                        <td className="px-3 py-2.5 align-top font-mono text-[11px]">
                          {t.username}@{t.sshHost}:{t.sshPort}
                        </td>
                        <td className="px-3 py-2.5 text-right align-top font-mono">
                          {t.connections}
                        </td>
                        <td className="px-3 py-2.5 text-right align-top font-mono text-[11px]">
                          <span className="text-sky-600">
                            ↑{formatBytes(t.bytesUp)}
                          </span>{" "}
                          <span className="text-emerald-600">
                            ↓{formatBytes(t.bytesDown)}
                          </span>
                        </td>
                        <td className="px-3 py-2.5 align-top">
                          <div className="flex items-center justify-end gap-0.5">
                            {active ? (
                              <Tooltip>
                                <TooltipTrigger asChild>
                                  <Button
                                    variant="ghost"
                                    size="icon"
                                    className="size-7"
                                    disabled={busy || t.state === "stopping"}
                                    onClick={() => stopTunnel(t.id)}
                                  >
                                    {busy ? (
                                      <Loader2 className="size-3.5 animate-spin" />
                                    ) : (
                                      <Square className="size-3.5 text-amber-600" />
                                    )}
                                  </Button>
                                </TooltipTrigger>
                                <TooltipContent>停止</TooltipContent>
                              </Tooltip>
                            ) : (
                              <Tooltip>
                                <TooltipTrigger asChild>
                                  <Button
                                    variant="ghost"
                                    size="icon"
                                    className="size-7"
                                    disabled={busy}
                                    onClick={() => startTunnel(t.id)}
                                  >
                                    {busy ? (
                                      <Loader2 className="size-3.5 animate-spin" />
                                    ) : (
                                      <Play className="size-3.5 text-emerald-600" />
                                    )}
                                  </Button>
                                </TooltipTrigger>
                                <TooltipContent>启动</TooltipContent>
                              </Tooltip>
                            )}
                            <Tooltip>
                              <TooltipTrigger asChild>
                                <Button
                                  variant="ghost"
                                  size="icon"
                                  className="size-7"
                                  onClick={() => openLogs(t)}
                                >
                                  <ScrollText className="size-3.5" />
                                </Button>
                              </TooltipTrigger>
                              <TooltipContent>日志</TooltipContent>
                            </Tooltip>
                            <Tooltip>
                              <TooltipTrigger asChild>
                                <Button
                                  variant="ghost"
                                  size="icon"
                                  className="size-7"
                                  disabled={active}
                                  onClick={() => {
                                    setEditing(t);
                                    setDialogOpen(true);
                                  }}
                                >
                                  <Pencil className="size-3.5" />
                                </Button>
                              </TooltipTrigger>
                              <TooltipContent>编辑</TooltipContent>
                            </Tooltip>
                            <Tooltip>
                              <TooltipTrigger asChild>
                                <Button
                                  variant="ghost"
                                  size="icon"
                                  className="size-7 hover:text-destructive"
                                  disabled={active || busy}
                                  onClick={() => {
                                    if (confirmDelete === t.id) {
                                      setConfirmDelete(null);
                                      deleteTunnel(t);
                                    } else {
                                      setConfirmDelete(t.id);
                                      setTimeout(
                                        () =>
                                          setConfirmDelete((c) =>
                                            c === t.id ? null : c
                                          ),
                                        3000
                                      );
                                    }
                                  }}
                                >
                                  {confirmDelete === t.id ? (
                                    <span className="text-[10px] text-destructive">
                                      确认?
                                    </span>
                                  ) : (
                                    <Trash2 className="size-3.5" />
                                  )}
                                </Button>
                              </TooltipTrigger>
                              <TooltipContent>删除</TooltipContent>
                            </Tooltip>
                          </div>
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
              <div className="border-t px-3 py-2 text-[11px] text-muted-foreground">
                共 {tunnels.length} 个隧道 · 日志最近更新时间{" "}
                {logLines.length
                  ? formatTime(logLines[logLines.length - 1].time * 1000)
                  : "--"}
              </div>
            </div>
          )}
        </CardContent>
      </Card>

      {/* ---- 弹窗 ---- */}
      <TunnelFormDialog
        open={dialogOpen}
        initial={editing}
        onClose={() => setDialogOpen(false)}
        onSaved={() => {
          refresh();
        }}
      />
      <TunnelLogDialog
        tunnel={logTunnel}
        logs={logLines}
        onClose={() => setLogTunnel(null)}
        onClear={clearLogs}
      />
    </div>
  );
}
