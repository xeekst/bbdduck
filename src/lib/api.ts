import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import type {
  JobSnapshot,
  RecentConnection,
  RemoteInfo,
  ServerConfig,
  ServerStatus,
  SyncOptions,
} from "./sync-types";

export async function pickFolder(): Promise<string | null> {
  const res = await open({ directory: true, multiple: false, title: "选择文件夹" });
  return typeof res === "string" ? res : null;
}

/**
 * Pick one or more folders at once. `defaultPath` lets the dialog open at a
 * specific directory (e.g. the parent of the previously chosen folder).
 */
export async function pickFolders(defaultPath?: string): Promise<string[]> {
  const res = await open({
    directory: true,
    multiple: true,
    title: "选择共享文件夹（可多选）",
    defaultPath,
  });
  if (Array.isArray(res)) return res;
  if (typeof res === "string") return [res];
  return [];
}

export const api = {
  serverStart: (ip: string, port: number, folders: string[]) =>
    invoke<ServerStatus>("server_start", { ip, port, folders }),
  serverStop: () => invoke<void>("server_stop"),
  serverStatus: () => invoke<ServerStatus>("server_status"),

  clientListShares: (ip: string, port: number) =>
    invoke<string[]>("client_list_shares", { ip, port }),
  clientRemoteInfo: (ip: string, port: number, share: string) =>
    invoke<RemoteInfo>("client_remote_info", { ip, port, share }),

  syncStart: (opts: SyncOptions) => invoke<string>("sync_start", { opts }),
  syncStop: (jobId: string) => invoke<void>("sync_stop", { jobId }),
  syncActiveJobs: () => invoke<JobSnapshot[]>("sync_active_jobs"),
  syncHistory: (limit: number) => invoke<JobSnapshot[]>("sync_history", { limit }),

  saveServerConfig: (name: string, ip: string, port: number, folders: string[]) =>
    invoke<number>("save_server_config", { name, ip, port, folders }),
  listServerConfigs: () => invoke<ServerConfig[]>("list_server_configs"),
  deleteServerConfig: (id: number) => invoke<void>("delete_server_config", { id }),

  saveRecentConnection: (ip: string, port: number, share: string, localDir: string) =>
    invoke<number>("save_recent_connection", { ip, port, share, localDir }),
  listRecentConnections: () => invoke<RecentConnection[]>("list_recent_connections"),
};
