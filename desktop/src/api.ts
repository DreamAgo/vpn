// Thin wrapper around the Tauri commands exposed by the Rust backend.
// Mirrors `vpn_cli::ipc::StatusResponse` / `ConnState`.
import { invoke } from "@tauri-apps/api/core";
import {
  disable as disableAutostart,
  enable as enableAutostart,
  isEnabled as isAutostartEnabled,
} from "@tauri-apps/plugin-autostart";
import { relaunch } from "@tauri-apps/plugin-process";
import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";
import { check, Update, type DownloadEvent } from "@tauri-apps/plugin-updater";

export type ConnState =
  | "disconnected"
  | "connecting"
  | "connected"
  | "reconnecting"
  | "error";

export interface StatusResponse {
  state: ConnState;
  vpn_ip: string | null;
  since: number | null;
  bytes_rx: number;
  bytes_tx: number;
  last_error: string | null;
}

export interface UpdateInfo {
  available: boolean;
  currentVersion: string | null;
  version: string | null;
  body: string | null;
}

export interface UpdateProgress {
  downloaded: number;
  total: number | null;
}

export interface DiagnosticsInfo {
  app_version: string;
  os: string;
  arch: string;
  log_dir: string | null;
  log_error: string | null;
}

let pendingUpdate: Update | null = null;
const PREVIEW_AUTOSTART_KEY = "yilian-preview-autostart";

function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

function previewStatus(): StatusResponse {
  return {
    state: "connected",
    vpn_ip: "10.8.0.12",
    since: Math.floor(Date.now() / 1000) - 42 * 60,
    bytes_rx: 84_213_840,
    bytes_tx: 12_884_021,
    last_error: null,
  };
}

export function getStatus(): Promise<StatusResponse> {
  if (!isTauriRuntime()) return Promise.resolve(previewStatus());
  return invoke<StatusResponse>("get_status");
}

export function getDiagnosticsInfo(): Promise<DiagnosticsInfo> {
  if (!isTauriRuntime()) {
    return Promise.resolve({
      app_version: "0.1.4",
      os: "preview",
      arch: "browser",
      log_dir: null,
      log_error: "预览模式不写本地日志",
    });
  }
  return invoke<DiagnosticsInfo>("diagnostics_info");
}

export function connect(): Promise<void> {
  if (!isTauriRuntime()) return Promise.resolve();
  return invoke<void>("connect");
}

export function disconnect(): Promise<void> {
  if (!isTauriRuntime()) return Promise.resolve();
  return invoke<void>("disconnect");
}

export function login(
  server: string,
  username: string,
  password: string,
): Promise<void> {
  if (!isTauriRuntime()) return Promise.resolve();
  return invoke<void>("login", { server, username, password });
}

export function logout(): Promise<void> {
  if (!isTauriRuntime()) return Promise.resolve();
  return invoke<void>("logout");
}

// 修改密码:成功后服务端吊销全部会话 → 调用方应登出并要求重新登录。
export function changePassword(
  currentPassword: string,
  newPassword: string,
): Promise<void> {
  if (!isTauriRuntime()) return Promise.resolve();
  return invoke<void>("change_password", { currentPassword, newPassword });
}

export function isLoggedIn(): Promise<boolean> {
  if (!isTauriRuntime()) return Promise.resolve(true);
  return invoke<boolean>("is_logged_in");
}

export function savedServer(): Promise<string | null> {
  if (!isTauriRuntime()) return Promise.resolve("https://access.example.com");
  return invoke<string | null>("saved_server");
}

export function hideWindow(): Promise<void> {
  if (!isTauriRuntime()) return Promise.resolve();
  return invoke<void>("hide_window");
}

export function quitApp(): Promise<void> {
  if (!isTauriRuntime()) return Promise.resolve();
  return invoke<void>("quit_app");
}

export function syncTrayState(state: ConnState): Promise<void> {
  if (!isTauriRuntime()) return Promise.resolve();
  return invoke<void>("sync_tray_state", { state });
}

export async function getLaunchOnStartup(): Promise<boolean> {
  if (!isTauriRuntime()) {
    return window.localStorage.getItem(PREVIEW_AUTOSTART_KEY) === "1";
  }
  return isAutostartEnabled();
}

export async function setLaunchOnStartup(enabled: boolean): Promise<void> {
  if (!isTauriRuntime()) {
    window.localStorage.setItem(PREVIEW_AUTOSTART_KEY, enabled ? "1" : "0");
    return;
  }
  if (enabled) {
    await enableAutostart();
  } else {
    await disableAutostart();
  }
}

export async function checkForUpdate(): Promise<UpdateInfo> {
  if (!isTauriRuntime()) {
    return {
      available: true,
      currentVersion: "0.1.0",
      version: "0.2.0",
      body: "客户端 UI 优化、连接状态反馈和自动更新体验改进。",
    };
  }

  const update = await check({ timeout: 10_000 });
  pendingUpdate = update;
  if (!update) {
    return {
      available: false,
      currentVersion: null,
      version: null,
      body: null,
    };
  }
  return {
    available: true,
    currentVersion: update.currentVersion,
    version: update.version,
    body: update.body ?? null,
  };
}

export async function installPendingUpdate(
  onProgress?: (progress: UpdateProgress) => void,
): Promise<void> {
  if (!isTauriRuntime()) {
    onProgress?.({ downloaded: 1, total: 3 });
    await new Promise((resolve) => window.setTimeout(resolve, 250));
    onProgress?.({ downloaded: 2, total: 3 });
    await new Promise((resolve) => window.setTimeout(resolve, 250));
    onProgress?.({ downloaded: 3, total: 3 });
    return;
  }

  const update = pendingUpdate ?? (await check({ timeout: 10_000 }));
  if (!update) throw new Error("当前没有可用更新");

  let downloaded = 0;
  let total: number | null = null;
  const handleEvent = (event: DownloadEvent) => {
    if (event.event === "Started") {
      downloaded = 0;
      total = event.data.contentLength ?? null;
      onProgress?.({ downloaded, total });
    } else if (event.event === "Progress") {
      downloaded += event.data.chunkLength;
      onProgress?.({ downloaded, total });
    } else if (event.event === "Finished") {
      onProgress?.({ downloaded: total ?? downloaded, total });
    }
  };

  await update.downloadAndInstall(handleEvent);
  pendingUpdate = null;
  await relaunch();
}

// 原生系统通知:首次按需申请权限并缓存结果;不可用时静默降级,绝不抛错打断轮询。
let permGranted: boolean | null = null;
export async function notify(title: string, body: string): Promise<void> {
  if (!isTauriRuntime()) return;

  try {
    if (permGranted === null) {
      permGranted = await isPermissionGranted();
      if (!permGranted) permGranted = (await requestPermission()) === "granted";
    }
    if (permGranted) sendNotification({ title, body });
  } catch {
    // 通知不可用(权限被拒/插件异常)时忽略。
  }
}
