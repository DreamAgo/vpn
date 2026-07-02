// Thin wrapper around the Tauri commands exposed by the Rust backend.
// Mirrors `vpn_cli::ipc::StatusResponse` / `ConnState`.
import { invoke } from "@tauri-apps/api/core";
import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";

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

export function getStatus(): Promise<StatusResponse> {
  return invoke<StatusResponse>("get_status");
}

export function connect(): Promise<void> {
  return invoke<void>("connect");
}

export function disconnect(): Promise<void> {
  return invoke<void>("disconnect");
}

export function login(
  server: string,
  username: string,
  password: string,
): Promise<void> {
  return invoke<void>("login", { server, username, password });
}

export function logout(): Promise<void> {
  return invoke<void>("logout");
}

// 修改密码:成功后服务端吊销全部会话 → 调用方应登出并要求重新登录。
export function changePassword(
  currentPassword: string,
  newPassword: string,
): Promise<void> {
  return invoke<void>("change_password", { currentPassword, newPassword });
}

export function isLoggedIn(): Promise<boolean> {
  return invoke<boolean>("is_logged_in");
}

export function savedServer(): Promise<string | null> {
  return invoke<string | null>("saved_server");
}

export function hideWindow(): Promise<void> {
  return invoke<void>("hide_window");
}

export function quitApp(): Promise<void> {
  return invoke<void>("quit_app");
}

// 原生系统通知:首次按需申请权限并缓存结果;不可用时静默降级,绝不抛错打断轮询。
let permGranted: boolean | null = null;
export async function notify(title: string, body: string): Promise<void> {
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
