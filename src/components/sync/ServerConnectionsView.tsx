import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { Activity, Clock3, FileUp, Network, Server, WifiOff } from "lucide-react";
import { api } from "@/lib/api";
import {
  EVT_SERVER,
  type ServerConnectionInfo,
  type ServerEvent,
  type ServerStatus,
} from "@/lib/sync-types";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Badge } from "@/components/ui/badge";
import { formatBytes } from "@/lib/utils";

function emptyStatus(): ServerStatus {
  return { running: false, addr: null, shares: [], connections: [] };
}

function connectionKindLabel(kind: ServerConnectionInfo["kind"]) {
  if (kind === "listing") return "目录扫描";
  if (kind === "transfer") return "文件传输";
  if (kind === "control") return "控制请求";
  if (kind === "error") return "连接错误";
  return "正在连接";
}

function formatTimestamp(seconds: number) {
  if (!Number.isFinite(seconds) || seconds <= 0) return "—";
  return new Date(seconds * 1000).toLocaleString("zh-CN", { hour12: false });
}

function ConnectionRow({ connection }: { connection: ServerConnectionInfo }) {
  const primaryActivity = connection.currentFile ?? connection.activity;

  return (
    <div className="rounded-md border bg-background/80 p-3 shadow-xs">
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        <Badge
          variant={connection.active ? "default" : "secondary"}
          className="h-5 px-1.5 text-[10px]"
        >
          {connection.active ? "活跃" : "已结束"}
        </Badge>
        <Badge
          variant={connection.kind === "error" ? "destructive" : "outline"}
          className="h-5 px-1.5 text-[10px]"
        >
          {connectionKindLabel(connection.kind)}
        </Badge>
        <Network className="size-3.5 shrink-0 text-muted-foreground" />
        <span className="min-w-0 font-mono text-xs font-medium" title={connection.peer}>
          {connection.peer}
        </span>
        <span className="ml-auto flex items-center gap-1 font-mono text-xs text-muted-foreground">
          <FileUp className="size-3.5" />
          已发送 {formatBytes(connection.bytesSent)}
        </span>
      </div>

      <div className="mt-3 grid gap-3 text-xs md:grid-cols-2 xl:grid-cols-[minmax(0,2fr)_minmax(0,1fr)_180px_180px]">
        <div className="min-w-0">
          <p className="mb-1 text-[10px] text-muted-foreground">当前文件 / 动作</p>
          <p className="truncate font-mono" title={primaryActivity}>
            {primaryActivity || "等待请求"}
          </p>
          {connection.currentFile && connection.activity && (
            <p className="mt-1 truncate text-[10px] text-muted-foreground" title={connection.activity}>
              {connection.activity}
            </p>
          )}
        </div>
        <div className="min-w-0">
          <p className="mb-1 text-[10px] text-muted-foreground">共享目录</p>
          <p className="truncate font-mono" title={connection.share ?? undefined}>
            {connection.share ?? "—"}
          </p>
        </div>
        <div>
          <p className="mb-1 flex items-center gap-1 text-[10px] text-muted-foreground">
            <Clock3 className="size-3" /> 连接时间
          </p>
          <p className="font-mono text-[11px]">{formatTimestamp(connection.connectedAt)}</p>
        </div>
        <div>
          <p className="mb-1 flex items-center gap-1 text-[10px] text-muted-foreground">
            <Activity className="size-3" /> 最后活动
          </p>
          <p className="font-mono text-[11px]">{formatTimestamp(connection.lastActiveAt)}</p>
        </div>
      </div>
    </div>
  );
}

export default function ServerConnectionsView() {
  const [status, setStatus] = useState<ServerStatus>(emptyStatus);
  const [refreshError, setRefreshError] = useState<string | null>(null);
  const [connectionTab, setConnectionTab] = useState<"active" | "ended">("active");

  useEffect(() => {
    let disposed = false;
    const refresh = () => {
      api.serverStatus()
        .then((next) => {
          if (disposed) return;
          setStatus(next);
          setRefreshError(null);
        })
        .catch((error) => {
          if (!disposed) setRefreshError(String(error));
        });
    };

    refresh();
    const timer = window.setInterval(refresh, 1000);
    const unlisten = listen<ServerEvent>(EVT_SERVER, (event) => {
      if (disposed) return;
      setStatus({
        running: event.payload.running,
        addr: event.payload.addr ?? null,
        shares: event.payload.shares ?? [],
        connections: event.payload.connections ?? [],
      });
      setRefreshError(event.payload.message ?? null);
    });

    return () => {
      disposed = true;
      window.clearInterval(timer);
      unlisten.then((fn) => fn());
    };
  }, []);

  if (!status.running) {
    return (
      <div className="flex h-full min-h-48 flex-col items-center justify-center rounded-md border border-dashed bg-muted/20 text-center">
        <WifiOff className="mb-3 size-8 text-muted-foreground/60" />
        <p className="text-sm font-medium">服务端尚未启动</p>
        <p className="mt-1 text-xs text-muted-foreground">
          请先在上方节点 A 配置共享目录并开启监听
        </p>
        {refreshError && <p className="mt-2 text-xs text-destructive">{refreshError}</p>}
      </div>
    );
  }

  const activeConnections = status.connections.filter((connection) => connection.active);
  const endedConnections = status.connections.filter((connection) => !connection.active);
  const activeCount = activeConnections.length;
  const endedCount = endedConnections.length;

  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden rounded-md border bg-muted/20">
      <div className="grid shrink-0 gap-2 border-b bg-background/60 p-3 sm:grid-cols-2 xl:grid-cols-4">
        <div className="flex min-w-0 items-center gap-2">
          <Server className="size-4 shrink-0 text-primary" />
          <div className="min-w-0">
            <p className="text-[10px] text-muted-foreground">监听地址</p>
            <p className="truncate font-mono text-xs" title={status.addr ?? undefined}>
              {status.addr ?? "—"}
            </p>
          </div>
        </div>
        <div>
          <p className="text-[10px] text-muted-foreground">客户端连接</p>
          <p className="text-xs">
            <span className="font-semibold text-primary">{activeCount}</span> 个活跃
            <span className="ml-2 text-muted-foreground">{endedCount} 个最近结束</span>
          </p>
        </div>
        <div>
          <p className="text-[10px] text-muted-foreground">共享目录</p>
          <p className="text-xs">{status.shares.length} 个</p>
        </div>
        <div className="min-w-0">
          <p className="text-[10px] text-muted-foreground">当前状态</p>
          <p className="truncate text-xs" title={status.shares.join(", ")}>
            {activeCount > 0 ? "正在响应客户端请求" : "监听中，等待客户端连接"}
          </p>
        </div>
      </div>

      <Tabs
        value={connectionTab}
        onValueChange={(value) => setConnectionTab(value as "active" | "ended")}
        className="min-h-0 flex-1 gap-0"
      >
        <div className="shrink-0 border-b bg-background/40 px-3 py-2">
          <TabsList className="h-8">
            <TabsTrigger value="active" className="gap-1.5 text-xs">
              活跃连接
              <Badge variant="secondary" className="h-4 min-w-4 px-1 text-[9px]">
                {activeCount}
              </Badge>
            </TabsTrigger>
            <TabsTrigger value="ended" className="gap-1.5 text-xs">
              已结束连接
              <Badge variant="secondary" className="h-4 min-w-4 px-1 text-[9px]">
                {endedCount}
              </Badge>
            </TabsTrigger>
          </TabsList>
        </div>

        <TabsContent value="active" className="mt-0 min-h-0 flex-1 overflow-hidden">
          <ConnectionList
            connections={activeConnections}
            emptyTitle="暂无活跃连接"
            emptyDescription="等待客户端扫描目录或请求传输文件"
          />
        </TabsContent>

        <TabsContent value="ended" className="mt-0 min-h-0 flex-1 overflow-hidden">
          <ConnectionList
            connections={endedConnections}
            emptyTitle="暂无已结束连接"
            emptyDescription="最近结束的连接会保留在这里"
          />
        </TabsContent>
      </Tabs>

      {refreshError && (
        <div className="shrink-0 border-t bg-destructive/5 px-3 py-1.5 text-xs text-destructive">
          {refreshError}
        </div>
      )}
    </div>
  );
}

function ConnectionList({
  connections,
  emptyTitle,
  emptyDescription,
}: {
  connections: ServerConnectionInfo[];
  emptyTitle: string;
  emptyDescription: string;
}) {
  return (
    <div className="h-full min-h-0 overflow-auto p-2">
      {connections.length === 0 ? (
        <div className="flex h-full min-h-36 flex-col items-center justify-center text-center">
          <Network className="mb-2 size-7 text-muted-foreground/60" />
          <p className="text-sm">{emptyTitle}</p>
          <p className="mt-1 text-xs text-muted-foreground">{emptyDescription}</p>
        </div>
      ) : (
        <div className="space-y-2">
          {connections.map((connection) => (
            <ConnectionRow key={connection.id} connection={connection} />
          ))}
        </div>
      )}
    </div>
  );
}
