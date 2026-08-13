// SSH 隧道数据流向示意图：节点 + 加密/明文连线 + 流动圆点动画。
// - FlowDiagram: 添加/编辑弹窗中的大幅示意图（标注各主机地位）
// - MiniFlow: 隧道表格行内的紧凑版流向

import {
  ArrowRight,
  Globe,
  Monitor,
  Network,
  Server,
  ShieldCheck,
} from "lucide-react";
import { cn } from "@/lib/utils";
import type { TunnelType } from "@/lib/ssh-types";

interface NodeDef {
  icon: React.ComponentType<{ className?: string }>;
  title: string;
  addr: string;
  role: string;
}

interface LineDef {
  label: string;
  encrypted: boolean;
}

function NodeBox({ def, active }: { def: NodeDef; active: boolean }) {
  return (
    <div className="flex min-w-[104px] shrink-0 flex-col items-center gap-1">
      <div
        className={cn(
          "flex w-full flex-col items-center gap-1 rounded-lg border px-3 py-2 transition-colors",
          active
            ? "border-primary/40 bg-primary/5"
            : "border-border bg-muted/30"
        )}
      >
        <def.icon className="size-4 text-primary" />
        <span className="text-xs font-semibold">{def.title}</span>
        <span className="max-w-full truncate font-mono text-[10px] text-muted-foreground">
          {def.addr}
        </span>
      </div>
      <span className="text-center text-[10px] leading-tight text-muted-foreground">
        {def.role}
      </span>
    </div>
  );
}

function Connector({
  def,
  animate,
  forward,
  compact,
}: {
  def: LineDef;
  animate: boolean;
  /** 主数据方向是否从左到右 */
  forward: boolean;
  compact?: boolean;
}) {
  const dotCls =
    "tunnel-flow-dot top-1/2 size-1.5 -translate-y-1/2 rounded-full";
  return (
    <div className={cn("flex flex-col items-center", compact ? "w-8" : "min-w-12 flex-1 px-1")}>
      <div className={cn("relative w-full", compact ? "h-3" : "h-6")}>
        <div
          className={cn(
            "tunnel-flow-line absolute inset-x-0 top-1/2 -translate-y-1/2",
            def.encrypted ? "text-sky-500" : "text-emerald-500",
            animate && "tunnel-pulse"
          )}
          style={{ backgroundSize: compact ? "8px 2px" : undefined }}
        />
        {animate && (
          <>
            <span
              className={cn(dotCls, "bg-sky-500")}
              style={{
                animationName: forward ? "tunnel-dot-lr" : "tunnel-dot-rl",
              }}
            />
            <span
              className={cn(dotCls, "bg-emerald-500")}
              style={{
                animationName: forward ? "tunnel-dot-rl" : "tunnel-dot-lr",
                animationDelay: "0.5s",
              }}
            />
          </>
        )}
      </div>
      {!compact && (
        <span className="mt-1 flex items-center gap-0.5 text-[10px] text-muted-foreground">
          {def.encrypted && <ShieldCheck className="size-3 text-sky-500" />}
          {def.label}
        </span>
      )}
    </div>
  );
}

export function FlowDiagram({
  tunnelType,
  proto = "tcp",
  listenAddr,
  sshAddr,
  targetAddr,
  running = false,
}: {
  tunnelType: TunnelType;
  proto?: "tcp" | "udp";
  listenAddr: string;
  sshAddr: string;
  targetAddr: string;
  running?: boolean;
}) {
  const protoLabel = proto.toUpperCase();
  let nodes: NodeDef[];
  let lines: LineDef[];
  let forward = true;

  if (tunnelType === "remote") {
    // SSH 服务器监听 → 本机 → 本地目标（主方向从 SSH 服务器回到本机）
    forward = false;
    nodes = [
      {
        icon: Server,
        title: "SSH 服务器",
        addr: listenAddr,
        role: "监听端 · 远端入口",
      },
      {
        icon: Monitor,
        title: "本机",
        addr: sshAddr,
        role: "隧道发起方 · 中转",
      },
      {
        icon: Network,
        title: "目标主机",
        addr: targetAddr,
        role: "本机可达的目标",
      },
    ];
    lines = [
      { label: "SSH 加密隧道", encrypted: true },
      { label: `${protoLabel} 明文`, encrypted: false },
    ];
  } else if (tunnelType === "dynamic") {
    forward = true;
    nodes = [
      {
        icon: Globe,
        title: "应用 / 客户端",
        addr: "任意 SOCKS5 客户端",
        role: "发起连接",
      },
      {
        icon: Monitor,
        title: "本机",
        addr: listenAddr,
        role: "SOCKS5 代理 · 隧道入口",
      },
      {
        icon: Server,
        title: "SSH 服务器",
        addr: sshAddr,
        role: "加密中转",
      },
      {
        icon: Network,
        title: "任意目标",
        addr: "由客户端指定",
        role: "按需连接",
      },
    ];
    lines = [
      { label: "SOCKS5", encrypted: false },
      { label: "SSH 加密隧道", encrypted: true },
      { label: "按需连接", encrypted: false },
    ];
  } else {
    forward = true;
    nodes = [
      {
        icon: Monitor,
        title: "本机",
        addr: listenAddr,
        role: "监听端 · 客户端入口",
      },
      {
        icon: Server,
        title: "SSH 服务器",
        addr: sshAddr,
        role: "加密中转 · 解密转发",
      },
      {
        icon: Network,
        title: "目标主机",
        addr: targetAddr,
        role: "最终目标（相对 SSH）",
      },
    ];
    lines = [
      { label: "SSH 加密隧道", encrypted: true },
      { label: `${protoLabel} 明文`, encrypted: false },
    ];
  }

  return (
    <div className="rounded-lg border bg-muted/20 px-3 py-2.5">
      <div className="flex items-start">
        {nodes.map((n, i) => (
          <div key={i} className="contents">
            {i > 0 && (
              <Connector def={lines[i - 1]} animate={running} forward={forward} />
            )}
            <NodeBox def={n} active={running && (i === 0 || i === nodes.length - 1)} />
          </div>
        ))}
      </div>
    </div>
  );
}

export function MiniFlow({ tunnelType }: { tunnelType: TunnelType }) {
  const steps =
    tunnelType === "remote"
      ? ["SSH 服务器", "本机", "目标"]
      : tunnelType === "dynamic"
        ? ["客户端", "本机", "SSH", "任意"]
        : ["本机", "SSH", "目标"];
  return (
    <div className="flex items-center gap-0.5 text-[11px] text-muted-foreground">
      {steps.map((s, i) => (
        <span key={i} className="flex items-center gap-0.5">
          {i > 0 && <ArrowRight className="size-3 text-sky-500" />}
          <span className={cn(i === 1 && "font-medium text-foreground")}>{s}</span>
        </span>
      ))}
    </div>
  );
}
