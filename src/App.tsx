import { useEffect, useRef, useState } from "react";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { AlertTriangle, Loader2 } from "lucide-react";
import { HashRouter, Navigate, Route, Routes } from "react-router-dom";
import { TooltipProvider } from "@/components/ui/tooltip";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import AppShell from "@/components/layout/AppShell";
import HomePage from "@/pages/HomePage";
import FileOccupancyPage from "@/pages/FileOccupancyPage";
import FileSyncPage from "@/pages/FileSyncPage";
import NetworkToolsPage from "@/pages/NetworkToolsPage";
import PortOccupancyPage from "@/pages/PortOccupancyPage";
import PortForwardPage from "@/pages/PortForwardPage";
import TcpConnectionStatsPage from "@/pages/TcpConnectionStatsPage";

function App() {
  const [closeConfirmOpen, setCloseConfirmOpen] = useState(false);
  const [closing, setClosing] = useState(false);
  const [closeError, setCloseError] = useState<string | null>(null);
  const allowCloseRef = useRef(false);

  useEffect(() => {
    let disposed = false;
    let unlisten: UnlistenFn | undefined;
    const appWindow = getCurrentWindow();

    void appWindow
      .onCloseRequested((event) => {
        if (allowCloseRef.current) return;
        event.preventDefault();
        setCloseError(null);
        setCloseConfirmOpen(true);
      })
      .then((removeListener) => {
        if (disposed) {
          removeListener();
        } else {
          unlisten = removeListener;
        }
      })
      .catch(() => {
        // 普通浏览器预览环境没有 Tauri 窗口事件，无需处理。
      });

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  const confirmClose = async () => {
    if (closing) return;
    setClosing(true);
    setCloseError(null);
    allowCloseRef.current = true;
    try {
      await getCurrentWindow().destroy();
    } catch (error) {
      allowCloseRef.current = false;
      setClosing(false);
      setCloseError(
        error instanceof Error ? error.message : `关闭应用失败：${String(error)}`
      );
    }
  };

  return (
    <TooltipProvider>
      <HashRouter>
        <Routes>
          <Route element={<AppShell />}>
            <Route path="/" element={<HomePage />} />
            <Route path="/sync" element={<FileSyncPage />} />
            <Route path="/file-occupancy" element={<FileOccupancyPage />} />
            <Route path="/network" element={<NetworkToolsPage />} />
            <Route path="/port-occupancy" element={<PortOccupancyPage />} />
            <Route path="/tcp-statistics" element={<TcpConnectionStatsPage />} />
            <Route path="/port-forward" element={<PortForwardPage />} />
            <Route path="*" element={<Navigate to="/" replace />} />
          </Route>
        </Routes>
      </HashRouter>

      <Dialog
        open={closeConfirmOpen}
        onOpenChange={(open) => {
          if (!closing) setCloseConfirmOpen(open);
        }}
      >
        <DialogContent showCloseButton={!closing}>
          <DialogHeader>
            <DialogTitle className="flex items-center gap-2">
              <AlertTriangle className="size-5 text-amber-600" />
              确认退出 bbdduck？
            </DialogTitle>
            <DialogDescription>
              退出后，正在运行的文件同步、共享监听和端口转发任务都会停止。
            </DialogDescription>
          </DialogHeader>
          {closeError && (
            <div className="rounded-md border border-destructive/30 bg-destructive/5 px-3 py-2 text-xs text-destructive">
              {closeError}
            </div>
          )}
          <DialogFooter>
            <Button
              variant="outline"
              disabled={closing}
              onClick={() => setCloseConfirmOpen(false)}
            >
              取消
            </Button>
            <Button
              variant="destructive"
              disabled={closing}
              onClick={() => void confirmClose()}
            >
              {closing && <Loader2 className="size-4 animate-spin" />}
              {closing ? "正在退出" : "确认退出"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </TooltipProvider>
  );
}

export default App;
