import { useState } from "react";
import { NavLink } from "react-router-dom";
import {
  Activity,
  ArrowLeftRight,
  ChevronDown,
  ChevronRight,
  FileSearch,
  FileText,
  FolderSync,
  PanelLeftClose,
  PanelLeftOpen,
  Radar,
  RefreshCw,
  Settings,
  Wifi,
} from "lucide-react";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";

interface NavItem {
  label: string;
  to: string;
  icon: React.ComponentType<{ className?: string }>;
  description?: string;
  disabled?: boolean;
}

interface NavGroup {
  id: string;
  label: string;
  icon: React.ComponentType<{ className?: string }>;
  items: NavItem[];
}

const GROUPS: NavGroup[] = [
  {
    id: "file-ops",
    label: "文件操作",
    icon: FileText,
    items: [
      {
        label: "文件同步",
        to: "/sync",
        icon: FolderSync,
        description: "两台服务器之间快速同步文件夹",
      },
      {
        label: "占用检测",
        to: "/file-occupancy",
        icon: FileSearch,
        description: "检测文件 Handle 占用并管理相关进程",
      },
    ],
  },
  {
    id: "network",
    label: "网络工具",
    icon: Wifi,
    items: [
      {
        label: "端口测试",
        to: "/network",
        icon: RefreshCw,
        description: "TCP 端口连通性检测与 Ping",
      },
      {
        label: "端口检测",
        to: "/port-occupancy",
        icon: Radar,
        description: "检测本机端口占用、监听 IP 与进程树",
      },
      {
        label: "TCP 连接统计",
        to: "/tcp-statistics",
        icon: Activity,
        description: "按端口和 IP 统计 TCP 状态与连接详情",
      },
      {
        label: "端口转发",
        to: "/port-forward",
        icon: ArrowLeftRight,
        description: "SSH TCP/UDP 隧道（本地/远程/动态）",
      },
    ],
  },
  {
    id: "system",
    label: "系统设置",
    icon: Settings,
    items: [
      {
        label: "偏好设置",
        to: "/settings",
        icon: Settings,
        disabled: true,
      },
    ],
  },
];

function Sidebar() {
  const [collapsed, setCollapsed] = useState(false);
  const [openGroups, setOpenGroups] = useState<Record<string, boolean>>({
    "file-ops": true,
  });

  const toggleGroup = (id: string) =>
    setOpenGroups((prev) => ({ ...prev, [id]: !prev[id] }));

  return (
    <aside
      className={cn(
        "flex shrink-0 flex-col border-r bg-sidebar text-sidebar-foreground transition-[width] duration-200",
        collapsed ? "w-12" : "w-56"
      )}
    >
      {/* Header */}
      <div
        className={cn(
          "flex h-11 items-center gap-2 border-b px-2",
          collapsed && "justify-center px-0"
        )}
      >
        {!collapsed && (
          <span className="flex-1 truncate px-1 text-sm font-semibold tracking-wide">
            bbdduck
          </span>
        )}
        <Button
          variant="ghost"
          size="icon"
          className="size-7"
          onClick={() => setCollapsed((c) => !c)}
          title={collapsed ? "展开侧边栏" : "收起侧边栏"}
        >
          {collapsed ? (
            <PanelLeftOpen className="size-4" />
          ) : (
            <PanelLeftClose className="size-4" />
          )}
        </Button>
      </div>

      {/* Groups */}
      <nav className="min-h-0 flex-1 overflow-y-auto overflow-x-hidden py-2">
        {GROUPS.map((group) => {
          if (collapsed) {
            return (
              <div key={group.id} className="mb-2 flex flex-col items-center gap-1">
                <span className="mb-1 w-6 border-t" />
                {group.items.map((item) => (
                  <Tooltip key={item.to}>
                    <TooltipTrigger asChild>
                      <span>
                        <NavLink
                          to={item.to}
                          className={({ isActive }) =>
                            cn(
                              "flex size-8 items-center justify-center rounded-md hover:bg-sidebar-accent hover:text-sidebar-accent-foreground",
                              isActive &&
                                "bg-sidebar-accent text-sidebar-accent-foreground"
                            )
                          }
                        >
                          <item.icon className="size-4" />
                        </NavLink>
                      </span>
                    </TooltipTrigger>
                    <TooltipContent side="right">{item.label}</TooltipContent>
                  </Tooltip>
                ))}
              </div>
            );
          }

          const open = openGroups[group.id] ?? false;
          return (
            <Collapsible
              key={group.id}
              open={open}
              onOpenChange={() => toggleGroup(group.id)}
            >
              <CollapsibleTrigger asChild>
                <button
                  className={cn(
                    "flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-sm font-semibold uppercase tracking-wide text-muted-foreground hover:bg-sidebar-accent hover:text-sidebar-accent-foreground"
                  )}
                >
                  {open ? (
                    <ChevronDown className="size-3.5 shrink-0" />
                  ) : (
                    <ChevronRight className="size-3.5 shrink-0" />
                  )}
                  <group.icon className="size-3.5 shrink-0" />
                  <span className="truncate">{group.label}</span>
                </button>
              </CollapsibleTrigger>
              <CollapsibleContent className="mt-0.5 space-y-0.5 pl-1 pr-1">
                {group.items.map((item) => (
                  <Tooltip key={item.to} delayDuration={600}>
                    <TooltipTrigger asChild>
                      <span className="block">
                        <NavLink
                          to={item.to}
                          aria-disabled={item.disabled}
                          title={item.description}
                          className={({ isActive }) =>
                            cn(
                              "flex items-center gap-2 rounded-md pl-8 pr-2 py-1 text-xs text-sidebar-foreground/90 hover:bg-sidebar-accent hover:text-sidebar-accent-foreground",
                              isActive &&
                                "bg-sidebar-accent text-sidebar-accent-foreground font-medium",
                              item.disabled &&
                                "pointer-events-none opacity-40"
                            )
                          }
                        >
                          <item.icon className="size-3.5 shrink-0" />
                          <span className="truncate">{item.label}</span>
                        </NavLink>
                      </span>
                    </TooltipTrigger>
                    {item.description && (
                      <TooltipContent side="right">
                        {item.description}
                      </TooltipContent>
                    )}
                  </Tooltip>
                ))}
              </CollapsibleContent>
            </Collapsible>
          );
        })}
      </nav>

      <div className="border-t p-2 text-center">
        <span className="text-[10px] text-muted-foreground">v0.1.0</span>
      </div>
    </aside>
  );
}

export default Sidebar;
