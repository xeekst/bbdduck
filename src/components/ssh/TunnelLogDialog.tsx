// 隧道日志弹窗：点击表格中「日志」按钮打开，实时追加当前隧道的日志。

import { Eraser } from "lucide-react";
import type { TunnelItem, TunnelLogEntry } from "@/lib/ssh-types";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import NetLogView from "@/components/net/NetLogView";

export default function TunnelLogDialog({
  tunnel,
  logs,
  onClose,
  onClear,
}: {
  tunnel: TunnelItem | null;
  logs: TunnelLogEntry[];
  onClose: () => void;
  onClear: () => void;
}) {
  return (
    <Dialog open={tunnel != null} onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="flex max-h-[80vh] flex-col gap-2 p-4 sm:max-w-3xl">
        <DialogHeader className="flex-row items-center justify-between space-y-0">
          <div className="grid gap-0.5 text-left">
            <DialogTitle className="text-sm">
              隧道日志 · {tunnel?.name}
            </DialogTitle>
            <DialogDescription className="text-xs">
              {tunnel?.listenAddr || `${tunnel?.listenHost}:${tunnel?.listenPort}`} · 连接、转发与错误记录
            </DialogDescription>
          </div>
          <Button
            variant="outline"
            size="sm"
            onClick={onClear}
            className="shrink-0"
          >
            <Eraser className="size-3.5" />
            清空
          </Button>
        </DialogHeader>
        <div className="h-[420px] overflow-hidden rounded-md border">
          <NetLogView
            logs={logs}
            title="隧道日志"
            emptyText="暂无日志，启动隧道后将在此展示连接过程"
          />
        </div>
      </DialogContent>
    </Dialog>
  );
}
