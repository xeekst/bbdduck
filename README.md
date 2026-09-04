# bbq-duck

bbq-duck 是一个面向桌面端的网络与文件操作工具，使用 Tauri 2 + React + TypeScript 构建。当前提供跨服务器文件夹同步、网络连通性诊断和 SSH 端口转发能力。

## 功能说明

### 1. 文件同步

文件同步采用“节点 A 共享、节点 B 拉取”的工作模式：

- 节点 A 在本机启动监听服务，选择一个或多个本地文件夹作为共享目录。
- 节点 B 输入节点 A 的 IP 和端口，读取共享目录列表并选择目标目录。
- 支持远端目录扫描、文件元信息获取和文件内容流式传输。
- 支持多线程下载，线程数限制在 1～64 之间。
- 支持总带宽限制；设置为 0 表示不限速。
- 支持增量同步：当本地文件大小相同且本地修改时间不早于远端文件时跳过传输。
- 支持镜像删除：可删除本地中远端已不存在的文件和目录，适合需要保持目录一致的场景。
- 下载失败后进入重试队列，采用延迟重试；超过最大重试次数后标记为失败，不影响其他文件继续传输。
- 支持停止任务，并实时显示任务统计、单文件进度、速度、已完成目录树、重试队列和日志。
- 支持保存节点 A 配置和最近连接记录，便于重复使用。

同步协议使用 TCP 长度前缀 JSON 消息进行握手、列出共享目录和传递文件元信息，文件内容随后以原始字节流传输。协议版本当前为 `1`。

### 2. 网络工具

- 查看本机主机名、操作系统、IPv4 网卡、地址、掩码、网关、DNS 和路由表。
- TCP 端口检测：解析目标主机并尝试建立 TCP 连接，显示耗时、解析地址、出口地址和匹配路由等信息。
- Ping：发送 1～10 次 ICMP Echo 请求，统计成功数、丢包率、平均/最小/最大 RTT。
- 检测过程会通过事件流输出实时日志，便于定位解析失败、连接超时、拒绝连接和路由问题。

### 3. SSH 端口转发

- 支持本地转发、远程转发和动态转发（SOCKS5）。
- TCP 和 UDP 隧道均可配置；动态转发提供 SOCKS5 CONNECT 能力。
- 支持密码认证和私钥认证，可配置私钥口令。
- 支持 SSH 保活、自动重连和多条隧道并行运行。
- 支持启动、停止、编辑、删除、全部启动和全部停止。
- 支持隧道状态、连接数、上下行流量、监听地址和运行时间展示。
- 支持实时隧道日志、历史日志读取和清空日志。
- 启用“应用启动时自动运行”的隧道会在应用启动后自动拉起。

### 4. 本地数据

应用使用 SQLite 保存本地数据，数据库文件名为 `bbq-duck.db`，位于 Tauri 的应用数据目录。首次运行新版本时会自动迁移旧版 `bbdduck.db` 和应用数据目录。当前保存：

- 节点 A 共享配置；
- 最近连接记录；
- 同步任务历史；
- SSH 隧道配置；
- SSH 隧道日志（每条隧道保留最近 2000 条）。

“偏好设置”目前只是导航占位入口，尚未实现具体设置项。

## 架构说明

### 总体架构

```text
┌─────────────────────────────────────────────────────────────┐
│                       Tauri Desktop App                      │
│                                                             │
│  React UI                                                   │
│  ├─ AppShell / Sidebar / StatusBar                         │
│  ├─ FileSyncPage                                            │
│  ├─ NetworkToolsPage                                        │
│  └─ PortForwardPage                                         │
│          │                                                   │
│          │ invoke(command) / listen(event)                  │
│          ▼                                                   │
│  TypeScript API Adapter + UI State                          │
│  ├─ src/lib/api.ts                                          │
│  ├─ src/lib/*-types.ts                                      │
│  └─ src/lib/syncStore.ts                                    │
│          │                                                   │
│          ▼                                                   │
│  Rust Tauri Command Layer                                   │
│  ├─ sync commands                                           │
│  ├─ network commands                                        │
│  ├─ SSH tunnel commands                                     │
│  └─ SQLite access                                            │
│          │                                                   │
│          ├──────── sync engine / TCP protocol                │
│          ├──────── network inspection / probe                │
│          ├──────── SSH runtime / forwarding                  │
│          └──────── SQLite (`bbq-duck.db`)                    │
└─────────────────────────────────────────────────────────────┘
```

### 前端层

- `src/App.tsx` 使用 `HashRouter` 注册首页、文件同步、网络工具和端口转发页面。
- `src/pages/` 负责页面级编排，不直接操作底层网络或文件系统。
- `src/components/` 放置页面组件和可复用 UI 组件；同步页面拆分为服务端面板、客户端面板、进度列表、完成目录树、重试队列和日志视图。
- `src/lib/api.ts` 是前端到 Tauri 的统一调用边界，集中封装 `invoke` 和文件夹选择对话框。
- `src/lib/*-types.ts` 与 Rust 的 Serde 数据结构保持 camelCase 对齐，定义命令返回值、事件载荷和页面状态类型。
- `src/lib/syncStore.ts` 使用外部 store 管理高频同步事件，并设置行数、目录树文件数和日志数上限，避免大量文件传输时拖慢 UI。

### Rust/Tauri 应用层

`src-tauri/src/lib.rs` 是应用组装和 Tauri 命令入口，创建并注入 `AppState`：

- `Db`：SQLite 数据库访问；
- `ServerHandle`：节点 A 的共享服务生命周期；
- `jobs`：正在运行的同步任务表，以 UUID 作为任务 ID；
- `TunnelManager`：SSH 隧道配置、运行时和事件管理。

前端通过 Tauri 命令调用后端，后端通过 Tauri 事件将长任务状态推送回前端。命令负责启动、停止、查询和持久化，耗时工作放在独立线程或 Tokio 异步任务中，避免阻塞 WebView。

### 文件同步子系统

```text
节点 A ServerHandle
        │ TCP
        │ Hello / ListShares / ListFiles / FetchFile
        ▼
节点 B Client + Sync Engine
        ├─ listing thread：扫描远端目录并生成下载任务
        ├─ download workers：多线程下载文件
        ├─ retry workers：处理失败文件和退避重试
        ├─ bandwidth limiter：限制总带宽
        └─ emitter thread：批量发送进度、完成文件和统计事件
```

同步引擎的关键行为：

1. 先创建本地目标目录并建立远端文件扫描任务。
2. 扫描线程把文件任务写入有界队列；增量模式会在入队前比较本地文件大小和修改时间。
3. 下载线程通过 `FetchFile` 获取文件元信息和内容，并将进度写入共享统计。
4. 失败文件进入独立重试队列；普通下载结束后，重试池扩展到完整线程规模以尽快排空队列。
5. 开启镜像删除且扫描未发生错误时，根据扫描到的相对路径集合删除本地多余项。
6. 任务结束后发送最终状态，并将汇总结果写入 SQLite。

协议层使用 `safe_join` 校验远端相对路径，拒绝绝对路径和 `..` 路径，避免文件请求逃逸共享根目录。

### SSH 隧道子系统

`ssh_tunnel/manager.rs` 负责配置加载、运行时实例、启停和自动启动；`runner.rs` 负责 SSH 连接、认证、保活、重连和数据转发；`model.rs` 定义隧道配置、状态和日志模型。

每条隧道都有独立运行时和停止信号。运行状态、错误、监听地址、连接数和流量通过 `ssh-tunnel-state` 等事件同步到前端，日志同时写入 SQLite 并实时推送到日志窗口。

### 网络工具子系统

`net_tool/mod.rs` 封装本机 IPv4 网络信息采集、DNS/地址解析、路由匹配、TCP 探测和 ICMP Ping。Tauri 命令通过 `spawn_blocking` 执行阻塞式网络操作，并使用网络日志事件向前端提供过程信息。

### 持久化与并发

- SQLite 连接由 `Mutex<Connection>` 保护，启用 WAL 和 `NORMAL` 同步模式。
- 同步任务的运行态保存在内存中，任务结束时持久化最终统计；历史查询来自 `sync_jobs` 表。
- 文件同步使用标准线程、`crossbeam-channel` 有界队列和原子停止标志。
- SSH 隧道使用 Tokio 多线程运行时，适合同时维护多条长连接。
- 所有跨边界数据通过 Serde 序列化，Rust 字段使用 `#[serde(rename_all = "camelCase")]` 与 TypeScript 类型对齐。

## 目录结构

```text
.
├─ src/                              # React 前端
│  ├─ pages/                         # 页面级组件
│  ├─ components/                   # 页面组件与 UI 组件
│  ├─ lib/api.ts                    # Tauri 命令封装
│  ├─ lib/*-types.ts                # 前后端共享数据形状
│  └─ lib/syncStore.ts              # 同步高频事件状态
├─ src-tauri/
│  ├─ src/lib.rs                    # Tauri 入口、状态和命令注册
│  ├─ src/db.rs                     # SQLite 持久化
│  ├─ src/sync/                     # 文件同步协议、服务端、客户端和引擎
│  ├─ src/ssh_tunnel/               # SSH 隧道模型、管理器和运行器
│  ├─ src/net_tool/                 # 本机网络信息和网络诊断
│  └─ tests/                        # Rust 协议、同步和隧道测试
├─ public/                          # 静态资源
├─ package.json                     # 前端和 Tauri 脚本
└─ vite.config.ts                   # Vite 配置
```

## 技术栈

- 桌面容器：Tauri 2
- 前端：React 19、TypeScript、Vite 7、React Router
- UI：Tailwind CSS 4、Radix UI、Lucide React
- 后端：Rust 2021、Tokio、Serde、Russh
- 网络与并发：TCP、ICMP、标准线程、crossbeam-channel、Tokio
- 数据库：SQLite（rusqlite bundled，WAL）
- 包管理：pnpm

## 开发与构建

环境要求：Node.js、pnpm、Rust、Cargo，以及 Tauri 2 对应的系统依赖。Windows 开发和打包可选择本机 WebView2 或固定版本运行时。

```bash
# 安装依赖
pnpm install

# 仅启动前端开发服务器
pnpm dev

# 启动 Tauri 桌面开发环境
pnpm tauri

# 使用固定版本 WebView2 启动开发环境
pnpm tauri:fixed

# 类型检查并构建前端 dist
pnpm build

# 构建 Tauri 应用
pnpm build:prod

# 携带固定版本 WebView2 构建
pnpm build:fixed

# 生成 Windows 可执行文件或安装包
pnpm build:exe
pnpm build:nsis
pnpm build:msi

# 运行 Rust 测试
cargo test --manifest-path src-tauri/Cargo.toml
```

## 测试覆盖

当前 Rust 测试覆盖以下关键路径：

- 同步协议握手、共享目录读取、文件列表流式传输和文件下载；
- 相对路径安全校验和路径穿越拒绝；
- 增量同步所需的修改时间读取；
- 同步引擎镜像删除和重试队列行为；
- SSH TCP 转发和 UDP 帧转发冒烟测试。

## 注意事项

- 文件同步服务端是应用内置的自定义 TCP 服务，不是 SMB/NFS；两端都需要运行 bbq-duck，节点 B 连接节点 A 的监听端口。
- 共享目录路径和 SSH 凭据会保存在本机 SQLite 中，请根据本机安全要求保护应用数据目录。
- 镜像删除是破坏性同步选项，启用前应确认远端目录是权威数据源，并做好备份。
- 防火墙、端口占用、网络隔离和 ICMP 权限可能导致端口检测或 Ping 失败。
- 目前没有独立的用户认证、TLS 加密或远端权限系统；文件同步监听端口和 SSH 配置应部署在可信网络环境中。
