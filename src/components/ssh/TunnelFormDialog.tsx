// 添加 / 编辑 SSH 隧道弹窗：三种隧道类型（本地 / 远程 / 动态），
// 顶部动画展示数据流向与各主机地位，支持密码 / 私钥两种认证。

import { useEffect, useState } from "react";
import { open as openFileDialog } from "@tauri-apps/plugin-dialog";
import { FolderKey, Loader2 } from "lucide-react";
import { api } from "@/lib/api";
import type {
  AuthKind,
  TunnelConfig,
  TunnelItem,
  TunnelProto,
  TunnelType,
} from "@/lib/ssh-types";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { FlowDiagram } from "@/components/ssh/FlowDiagram";

interface FormState {
  name: string;
  tunnelType: TunnelType;
  proto: TunnelProto;
  sshHost: string;
  sshPort: string;
  username: string;
  auth: AuthKind;
  password: string;
  keyPath: string;
  keyPassphrase: string;
  listenHost: string;
  listenPort: string;
  targetHost: string;
  targetPort: string;
  keepaliveSecs: string;
  autoReconnect: boolean;
  enabled: boolean;
}

function defaultForm(): FormState {
  return {
    name: "",
    tunnelType: "local",
    proto: "tcp",
    sshHost: "",
    sshPort: "22",
    username: "",
    auth: "password",
    password: "",
    keyPath: "",
    keyPassphrase: "",
    listenHost: "127.0.0.1",
    listenPort: "",
    targetHost: "",
    targetPort: "",
    keepaliveSecs: "30",
    autoReconnect: true,
    enabled: false,
  };
}

function fromItem(item: TunnelItem): FormState {
  return {
    name: item.name,
    tunnelType: item.tunnelType,
    proto: item.proto,
    sshHost: item.sshHost,
    sshPort: String(item.sshPort),
    username: item.username,
    auth: item.auth,
    password: item.password ?? "",
    keyPath: item.keyPath ?? "",
    keyPassphrase: item.keyPassphrase ?? "",
    listenHost: item.listenHost,
    listenPort: String(item.listenPort),
    targetHost: item.targetHost,
    targetPort: item.targetPort ? String(item.targetPort) : "",
    keepaliveSecs: String(item.keepaliveSecs),
    autoReconnect: item.autoReconnect,
    enabled: item.enabled,
  };
}

function Field({
  label,
  children,
  className,
}: {
  label: string;
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <div className={cn("grid gap-1.5", className)}>
      <Label className="text-xs text-muted-foreground">{label}</Label>
      {children}
    </div>
  );
}

function parsePort(s: string): number {
  const n = parseInt(s, 10);
  if (!Number.isInteger(n) || n < 1 || n > 65535) {
    throw new Error(`端口号无效：${s}`);
  }
  return n;
}

export default function TunnelFormDialog({
  open,
  initial,
  onClose,
  onSaved,
}: {
  open: boolean;
  initial: TunnelItem | null;
  onClose: () => void;
  onSaved: (item: TunnelItem) => void;
}) {
  const [form, setForm] = useState<FormState>(defaultForm());
  const [err, setErr] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (open) {
      setForm(initial ? fromItem(initial) : defaultForm());
      setErr(null);
      setSaving(false);
    }
  }, [open, initial]);

  const patch = (p: Partial<FormState>) => setForm((f) => ({ ...f, ...p }));

  const pickKey = async () => {
    const res = await openFileDialog({
      multiple: false,
      title: "选择 SSH 私钥文件",
    });
    if (typeof res === "string") patch({ keyPath: res });
  };

  const listenAddr =
    (form.listenHost || "127.0.0.1") + ":" + (form.listenPort || "?");
  const sshAddr =
    (form.username || "user") +
    "@" +
    (form.sshHost || "ssh-host") +
    ":" +
    (form.sshPort || "22");
  const targetAddr =
    (form.targetHost || "target") + ":" + (form.targetPort || "?");

  const save = async () => {
    setErr(null);
    try {
      const dynamic = form.tunnelType === "dynamic";
      const config: TunnelConfig = {
        id: initial?.id ?? 0,
        name: form.name.trim(),
        tunnelType: form.tunnelType,
        proto: dynamic ? "tcp" : form.proto,
        sshHost: form.sshHost.trim(),
        sshPort: parsePort(form.sshPort),
        username: form.username.trim(),
        auth: form.auth,
        password: form.auth === "password" ? form.password : null,
        keyPath: form.auth === "key" ? form.keyPath.trim() : null,
        keyPassphrase:
          form.auth === "key" && form.keyPassphrase
            ? form.keyPassphrase
            : null,
        listenHost: form.listenHost.trim() || "127.0.0.1",
        listenPort: parsePort(form.listenPort),
        targetHost: dynamic ? "" : form.targetHost.trim(),
        targetPort: dynamic ? 0 : parsePort(form.targetPort),
        keepaliveSecs: Math.max(
          5,
          Math.min(3600, parseInt(form.keepaliveSecs, 10) || 30)
        ),
        autoReconnect: form.autoReconnect,
        enabled: form.enabled,
        createdAt: initial?.createdAt ?? 0,
      };
      if (!config.name) throw new Error("请填写隧道名称");
      if (!config.sshHost) throw new Error("请填写 SSH 主机");
      if (!config.username) throw new Error("请填写 SSH 用户名");
      if (!dynamic && !config.targetHost) throw new Error("请填写目标主机");
      if (config.auth === "password" && !config.password)
        throw new Error("请填写 SSH 密码");
      if (config.auth === "key" && !config.keyPath)
        throw new Error("请选择私钥文件");

      setSaving(true);
      const item = await api.sshTunnelSave(config);
      onSaved(item);
      onClose();
    } catch (e) {
      setErr(String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="max-h-[92vh] overflow-y-auto sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>{initial ? "编辑隧道" : "添加隧道"}</DialogTitle>
          <DialogDescription>
            本地 / 远程 / 动态三种隧道，支持 TCP 与 UDP；配置保存在本地 SQLite。
          </DialogDescription>
        </DialogHeader>

        {/* 数据流向动画 */}
        <FlowDiagram
          tunnelType={form.tunnelType}
          proto={form.proto}
          listenAddr={listenAddr}
          sshAddr={sshAddr}
          targetAddr={targetAddr}
          running
        />

        <Tabs
          value={form.tunnelType}
          onValueChange={(v) =>
            patch({
              tunnelType: v as TunnelType,
              proto: v === "dynamic" ? "tcp" : form.proto,
            })
          }
        >
          <TabsList className="grid w-full grid-cols-3">
            <TabsTrigger value="local">本地转发</TabsTrigger>
            <TabsTrigger value="remote">远程转发</TabsTrigger>
            <TabsTrigger value="dynamic">动态转发</TabsTrigger>
          </TabsList>
        </Tabs>

        <div className="grid gap-3">
          <Field label="隧道名称">
            <Input
              value={form.name}
              onChange={(e) => patch({ name: e.target.value })}
              placeholder="例如：数据库跳板"
            />
          </Field>

          {/* SSH 服务器 */}
          <div className="rounded-lg border p-3">
            <p className="mb-2 text-xs font-semibold text-muted-foreground">
              SSH 服务器
            </p>
            <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
              <Field label="主机" className="col-span-2">
                <Input
                  value={form.sshHost}
                  onChange={(e) => patch({ sshHost: e.target.value })}
                  placeholder="ssh.example.com"
                />
              </Field>
              <Field label="端口">
                <Input
                  value={form.sshPort}
                  onChange={(e) => patch({ sshPort: e.target.value })}
                />
              </Field>
              <Field label="用户名">
                <Input
                  value={form.username}
                  onChange={(e) => patch({ username: e.target.value })}
                  placeholder="root"
                />
              </Field>
              <Field label="认证方式">
                <Select
                  value={form.auth}
                  onValueChange={(v) => patch({ auth: v as AuthKind })}
                >
                  <SelectTrigger size="sm" className="w-full">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="password">密码</SelectItem>
                    <SelectItem value="key">私钥</SelectItem>
                  </SelectContent>
                </Select>
              </Field>
              {form.auth === "password" ? (
                <Field label="密码" className="col-span-3">
                  <Input
                    type="password"
                    value={form.password}
                    onChange={(e) => patch({ password: e.target.value })}
                    placeholder="保存在本机 SQLite 中"
                  />
                </Field>
              ) : (
                <>
                  <Field label="私钥路径" className="col-span-3">
                    <div className="flex gap-1.5">
                      <Input
                        value={form.keyPath}
                        onChange={(e) => patch({ keyPath: e.target.value })}
                        placeholder="C:\Users\you\.ssh\id_ed25519"
                      />
                      <Button
                        type="button"
                        variant="outline"
                        size="icon"
                        className="size-8 shrink-0"
                        onClick={pickKey}
                        title="选择私钥文件"
                      >
                        <FolderKey className="size-4" />
                      </Button>
                    </div>
                  </Field>
                  <Field label="私钥口令（可选）" className="col-span-3">
                    <Input
                      type="password"
                      value={form.keyPassphrase}
                      onChange={(e) =>
                        patch({ keyPassphrase: e.target.value })
                      }
                      placeholder="私钥加密口令，无则留空"
                    />
                  </Field>
                </>
              )}
            </div>
          </div>

          {/* 转发配置 */}
          <div className="rounded-lg border p-3">
            <p className="mb-2 text-xs font-semibold text-muted-foreground">
              转发配置
            </p>
            {form.tunnelType === "local" && (
              <div className="grid grid-cols-2 gap-3 sm:grid-cols-5">
                <Field label="协议">
                  <Select
                    value={form.proto}
                    onValueChange={(v) => patch({ proto: v as TunnelProto })}
                  >
                    <SelectTrigger size="sm" className="w-full">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="tcp">TCP</SelectItem>
                      <SelectItem value="udp">UDP</SelectItem>
                    </SelectContent>
                  </Select>
                </Field>
                <Field label="本地监听地址">
                  <Input
                    value={form.listenHost}
                    onChange={(e) => patch({ listenHost: e.target.value })}
                    placeholder="127.0.0.1"
                  />
                </Field>
                <Field label="本地监听端口">
                  <Input
                    value={form.listenPort}
                    onChange={(e) => patch({ listenPort: e.target.value })}
                    placeholder="8080"
                  />
                </Field>
                <Field label="目标主机（相对 SSH 服务器）">
                  <Input
                    value={form.targetHost}
                    onChange={(e) => patch({ targetHost: e.target.value })}
                    placeholder="10.0.0.5"
                  />
                </Field>
                <Field label="目标端口">
                  <Input
                    value={form.targetPort}
                    onChange={(e) => patch({ targetPort: e.target.value })}
                    placeholder="80"
                  />
                </Field>
              </div>
            )}
            {form.tunnelType === "remote" && (
              <div className="grid grid-cols-2 gap-3 sm:grid-cols-5">
                <Field label="协议">
                  <Select
                    value={form.proto}
                    onValueChange={(v) => patch({ proto: v as TunnelProto })}
                  >
                    <SelectTrigger size="sm" className="w-full">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="tcp">TCP</SelectItem>
                      <SelectItem value="udp">UDP</SelectItem>
                    </SelectContent>
                  </Select>
                </Field>
                <Field label="SSH 服务器监听地址">
                  <Input
                    value={form.listenHost}
                    onChange={(e) => patch({ listenHost: e.target.value })}
                    placeholder="127.0.0.1"
                  />
                </Field>
                <Field label="SSH 服务器监听端口">
                  <Input
                    value={form.listenPort}
                    onChange={(e) => patch({ listenPort: e.target.value })}
                    placeholder="9000"
                  />
                </Field>
                <Field label="目标主机（本机可达）">
                  <Input
                    value={form.targetHost}
                    onChange={(e) => patch({ targetHost: e.target.value })}
                    placeholder="127.0.0.1"
                  />
                </Field>
                <Field label="目标端口">
                  <Input
                    value={form.targetPort}
                    onChange={(e) => patch({ targetPort: e.target.value })}
                    placeholder="3389"
                  />
                </Field>
              </div>
            )}
            {form.tunnelType === "dynamic" && (
              <div className="grid grid-cols-2 gap-3 sm:grid-cols-2">
                <Field label="本地 SOCKS5 监听地址">
                  <Input
                    value={form.listenHost}
                    onChange={(e) => patch({ listenHost: e.target.value })}
                    placeholder="127.0.0.1"
                  />
                </Field>
                <Field label="本地 SOCKS5 监听端口">
                  <Input
                    value={form.listenPort}
                    onChange={(e) => patch({ listenPort: e.target.value })}
                    placeholder="1080"
                  />
                </Field>
              </div>
            )}
            <p className="mt-2 text-[11px] leading-relaxed text-muted-foreground">
              {form.tunnelType === "local" &&
                "访问本机监听端口即可通过 SSH 访问服务器内网目标。UDP 通过 SSH 通道内的轻量中继实现，需远程主机装有 python3/python。"}
              {form.tunnelType === "remote" &&
                "SSH 服务器将监听指定端口并把连接转发回本机。监听非 127.0.0.1 地址需服务器 sshd 允许 GatewayPorts。"}
              {form.tunnelType === "dynamic" &&
                "本机作为 SOCKS5 代理（无认证），浏览器/应用配置该代理后可按需经 SSH 访问任意目标。"}
            </p>
          </div>

          {/* 高级选项 */}
          <div className="flex flex-wrap items-center gap-x-6 gap-y-3 rounded-lg border p-3">
            <Field label="保活间隔（秒）" className="w-36">
              <Input
                value={form.keepaliveSecs}
                onChange={(e) => patch({ keepaliveSecs: e.target.value })}
              />
            </Field>
            <div className="flex items-center gap-2">
              <Switch
                checked={form.autoReconnect}
                onCheckedChange={(v) => patch({ autoReconnect: v })}
                id="tunnel-reconnect"
              />
              <Label htmlFor="tunnel-reconnect" className="text-xs">
                断线自动重连
              </Label>
            </div>
            <div className="flex items-center gap-2">
              <Switch
                checked={form.enabled}
                onCheckedChange={(v) => patch({ enabled: v })}
                id="tunnel-enabled"
              />
              <Label htmlFor="tunnel-enabled" className="text-xs">
                随应用启动
              </Label>
            </div>
          </div>

          {err && (
            <div className="rounded-md border border-destructive/40 bg-destructive/5 px-3 py-2 text-xs text-destructive">
              {err}
            </div>
          )}
        </div>

        <DialogFooter>
          <Button variant="outline" onClick={onClose} disabled={saving}>
            取消
          </Button>
          <Button onClick={save} disabled={saving}>
            {saving && <Loader2 className="size-4 animate-spin" />}
            保存
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
