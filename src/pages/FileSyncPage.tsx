import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import { listen } from "@tauri-apps/api/event";
import { FolderSync } from "lucide-react";
import { api } from "@/lib/api";
import {
  EVT_FILES_DELETED,
  EVT_FILES_DONE,
  EVT_JOB,
  EVT_LOG,
  EVT_PROGRESS,
  EVT_RETRY,
  type FileProgressEvent,
  type FilesDeletedEvent,
  type FilesDoneEvent,
  type JobEvent,
  type LogEvent,
  type RetryEvent,
  type SyncOptions,
} from "@/lib/sync-types";
import { syncStore } from "@/lib/syncStore";
import { Badge } from "@/components/ui/badge";
import { Card, CardContent, CardHeader } from "@/components/ui/card";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import ServerPanel from "@/components/sync/ServerPanel";
import ClientPanel from "@/components/sync/ClientPanel";
import SyncSummaryBar from "@/components/sync/SyncSummaryBar";
import SyncProgressList from "@/components/sync/SyncProgressList";
import CompletedTreeView from "@/components/sync/CompletedTreeView";
import RetryQueueView from "@/components/sync/RetryQueueView";
import SyncLogView from "@/components/sync/SyncLogView";

function TabCount({ kind }: { kind: "active" | "done" | "retry" }) {
  useSyncExternalStore(syncStore.subscribe, syncStore.getSnapshot);
  const n =
    kind === "active"
      ? syncStore.activeCount
      : kind === "done"
        ? syncStore.tree.count
        : syncStore.retries.size;
  if (n <= 0) return null;
  return (
    <Badge variant="secondary" className="ml-1 rounded-full px-1.5 text-[10px] font-normal">
      {n >= 1000 ? `${Math.floor(n / 1000)}k+` : n}
    </Badge>
  );
}

function FileSyncPage() {
  const jobIdRef = useRef<string | null>(null);
  // Once the job has reached a terminal state (stopped/finished/error), ignore
  // any late "running" events for that job so the UI can't flip back to
  // "同步中" after a stop.
  const terminalRef = useRef(false);
  const [running, setRunning] = useState(false);
  const [syncing, setSyncing] = useState(false);
  const [stopping, setStopping] = useState(false);
  const [startedAt, setStartedAt] = useState<number | null>(null);
  const [collapsed, setCollapsed] = useState(false);

  useEffect(() => {
    const un: Promise<() => void>[] = [];

    un.push(
      listen<JobEvent>(EVT_JOB, (e) => {
        const job = e.payload;
        if (jobIdRef.current && job.id !== jobIdRef.current) return;
        if (job.status === "running") {
          if (terminalRef.current) return; // stale running event after stop
          setRunning(true);
          setSyncing(false);
          syncStore.setJob(job);
        } else {
          terminalRef.current = true;
          setRunning(false);
          setSyncing(false);
          setStopping(false);
          // Keep the final terminal job so the summary shows 已完成/已停止/出错
          // with its final stats (e.g. "跳过 N" when everything was up to date)
          // instead of instantly resetting to "尚未开始同步".
          syncStore.setJob(job);
          syncStore.finish();
        }
      })
    );

    un.push(
      listen<FileProgressEvent>(EVT_PROGRESS, (e) => {
        if (e.payload.id !== jobIdRef.current) return;
        syncStore.upsertProgress(e.payload);
      })
    );

    un.push(
      listen<FilesDoneEvent>(EVT_FILES_DONE, (e) => {
        if (e.payload.id !== jobIdRef.current) return;
        syncStore.addFilesDone(e.payload.files);
      })
    );

    un.push(
      listen<FilesDeletedEvent>(EVT_FILES_DELETED, (e) => {
        if (e.payload.id !== jobIdRef.current) return;
        syncStore.addDeleted(e.payload.files);
      })
    );

    un.push(
      listen<RetryEvent>(EVT_RETRY, (e) => {
        if (e.payload.id !== jobIdRef.current) return;
        syncStore.upsertRetry(e.payload);
      })
    );

    un.push(
      listen<LogEvent>(EVT_LOG, (e) => {
        if (e.payload.id !== jobIdRef.current) return;
        syncStore.addLog({
          time: e.payload.time * 1000,
          level: e.payload.level,
          message: e.payload.message,
        });
      })
    );

    return () => {
      un.forEach((p) => p.then((fn) => fn()));
    };
  }, []);

  const handleStart = async (opts: SyncOptions) => {
    const jid = await api.syncStart(opts);
    jobIdRef.current = jid;
    terminalRef.current = false; // new job: allow running events again
    syncStore.reset(jid, {
      share: opts.share,
      localDir: opts.localDir,
      startedAt: Date.now(),
    });
    setRunning(false);
    setSyncing(true);
    setStopping(false);
    setStartedAt(Date.now());
  };

  const handleStop = async () => {
    if (!jobIdRef.current || stopping) return;
    setStopping(true);
    // Mark terminal locally so any late "running" event cannot re-arm the UI
    // while the backend is still unwinding.
    terminalRef.current = true;
    try {
      await api.syncStop(jobIdRef.current);
    } catch {
      /* ignore */
    }
  };

  return (
    <div className="flex h-full flex-col gap-4 p-4">
      <div className="flex items-center gap-2">
        <FolderSync className="size-5 text-primary" />
        <h1 className="text-lg font-semibold">文件同步</h1>
        <p className="text-xs text-muted-foreground">
          两台服务器快速同步文件夹：一台监听共享，另一台连接并同步到本机
        </p>
      </div>

      <div className="grid grid-cols-1 gap-4 xl:grid-cols-2">
        <ServerPanel collapsed={collapsed} onToggleCollapsed={() => setCollapsed((c) => !c)} />
        <ClientPanel
          running={running}
          syncing={syncing}
          stopping={stopping}
          collapsed={collapsed}
          onToggleCollapsed={() => setCollapsed((c) => !c)}
          onStart={handleStart}
          onStop={handleStop}
        />
      </div>

      <Card className="flex min-h-0 flex-1 flex-col gap-0 py-3">
        <CardHeader className="px-4 py-0">
          <SyncSummaryBar startedAt={startedAt} />
        </CardHeader>
        <CardContent className="flex min-h-0 flex-1 flex-col gap-0 px-4 pt-2">
          <Tabs defaultValue="transfer" className="flex min-h-0 flex-1 flex-col gap-2">
            <TabsList>
              <TabsTrigger value="transfer">
                正在传输
                <TabCount kind="active" />
              </TabsTrigger>
              <TabsTrigger value="done">
                已完成目录
                <TabCount kind="done" />
              </TabsTrigger>
              <TabsTrigger value="retry">
                重试队列
                <TabCount kind="retry" />
              </TabsTrigger>
              <TabsTrigger value="log">日志</TabsTrigger>
            </TabsList>

            <TabsContent
              value="transfer"
              className="relative min-h-0 flex-1 overflow-hidden rounded-md border bg-muted/20"
            >
              <div className="absolute inset-0">
                <SyncProgressList />
              </div>
            </TabsContent>

            <TabsContent
              value="done"
              className="relative min-h-0 flex-1 overflow-hidden rounded-md border bg-muted/20"
            >
              <div className="absolute inset-0">
                <CompletedTreeView />
              </div>
            </TabsContent>

            <TabsContent
              value="retry"
              className="relative min-h-0 flex-1 overflow-hidden rounded-md border bg-muted/20"
            >
              <div className="absolute inset-0">
                <RetryQueueView />
              </div>
            </TabsContent>

            <TabsContent
              value="log"
              className="relative min-h-0 flex-1 overflow-hidden rounded-md border bg-muted/20"
            >
              <div className="absolute inset-0">
                <SyncLogView />
              </div>
            </TabsContent>
          </Tabs>
        </CardContent>
      </Card>
    </div>
  );
}

export default FileSyncPage;
