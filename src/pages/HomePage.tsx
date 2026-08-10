import { Link } from "react-router-dom";
import { ArrowRight, FolderSync, HardDrive, Network, Zap } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";

const FEATURES = [
  {
    icon: FolderSync,
    title: "文件同步",
    desc: "两台服务器之间快速同步文件夹，支持多线程、带宽限制与增量同步。",
  },
  {
    icon: Network,
    title: "节点模式",
    desc: "一台作为节点 A 开启监听共享文件夹，另一台输入 IP 端口即可连接。",
  },
  {
    icon: Zap,
    title: "高性能渲染",
    desc: "海量文件进度窗口化展示，实时显示当前传输文件与完成目录树。",
  },
  {
    icon: HardDrive,
    title: "本地存储",
    desc: "所有配置与同步历史通过 SQLite 保存在本地。",
  },
];

function HomePage() {
  return (
    <div className="flex h-full flex-col items-center justify-center gap-8 p-8">
      <div className="text-center">
        <h1 className="text-3xl font-bold tracking-tight">bbdduck</h1>
        <p className="mt-2 text-sm text-muted-foreground">
          跨服务器文件夹快速同步工具 · Tauri + React + shadcn/ui
        </p>
      </div>

      <div className="grid w-full max-w-3xl grid-cols-1 gap-4 sm:grid-cols-2">
        {FEATURES.map((f) => (
          <Card key={f.title} className="gap-3 py-4">
            <CardHeader className="px-5 py-0">
              <CardTitle className="flex items-center gap-2 text-base">
                <f.icon className="size-4 text-primary" />
                {f.title}
              </CardTitle>
            </CardHeader>
            <CardContent className="px-5 text-sm text-muted-foreground">
              {f.desc}
            </CardContent>
          </Card>
        ))}
      </div>

      <Button asChild size="lg">
        <Link to="/sync">
          打开文件同步
          <ArrowRight />
        </Link>
      </Button>
    </div>
  );
}

export default HomePage;
