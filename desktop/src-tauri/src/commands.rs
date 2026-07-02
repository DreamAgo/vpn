//! Tauri commands: thin bridge from the React front-end to the in-process VPN
//! connection manager ([`crate::manager::VpnManager`]) and the credential store.
//!
//! 单进程架构:`connect`/`disconnect`/`get_status` 直接驱动本进程内的用户态
//! 隧道(库调用 `vpn-cli`),不经独立 daemon / IPC。`login`/`logout` 仍走凭证存储。

use std::sync::Arc;

use vpn_cli::api::ApiClient;
use vpn_cli::cli::{run_login, run_logout};
use vpn_cli::config::CredentialRepo;
use vpn_cli::ipc::StatusResponse;

use crate::manager::VpnManager;

/// Open a file-backed credential repo (most reliable, no keyring prompts).
fn repo() -> Result<CredentialRepo, String> {
    CredentialRepo::file().map_err(|e| e.to_string())
}

/// 当前连接状态(前端每 2.5s 轮询)。读本进程内状态,不会失败。
#[tauri::command]
pub async fn get_status(mgr: tauri::State<'_, Arc<VpnManager>>) -> Result<StatusResponse, ()> {
    Ok(mgr.status().await)
}

/// 建立连接(注册 + 建用户态隧道 + 心跳)。需以特权运行(开 TUN)。
#[tauri::command]
pub async fn connect(mgr: tauri::State<'_, Arc<VpnManager>>) -> Result<(), String> {
    mgr.connect().await
}

/// 断开连接。
#[tauri::command]
pub async fn disconnect(mgr: tauri::State<'_, Arc<VpnManager>>) -> Result<(), String> {
    mgr.disconnect().await
}

/// Log in and persist credentials. `routes` intentionally empty (desktop client
/// is not a site-gateway). Returns Err(message) on failure.
#[tauri::command]
pub async fn login(server: String, username: String, password: String) -> Result<(), String> {
    let repo = repo()?;
    run_login(&server, Some(&username), Some(&password), &[], &repo)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// 修改当前登录用户的密码。用本地存的 refresh token 换取 access 后调用服务端
/// change-password。**注意**:服务端会吊销该用户全部会话,故修改成功后本地凭证失效,
/// 前端应据此登出并要求用新密码重新登录。
#[tauri::command]
pub async fn change_password(current_password: String, new_password: String) -> Result<(), String> {
    let repo = repo()?;
    let server = repo
        .server_url()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "未登录".to_string())?;
    let refresh = repo
        .refresh_token()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "未登录".to_string())?;
    let api = ApiClient::new(&server).map_err(|e| e.to_string())?;
    api.set_refresh_token(refresh);
    // 先用 refresh 取一个 access token,再改密码。
    api.refresh().await.map_err(|e| e.to_string())?;
    api.change_password(&current_password, &new_password)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn logout(mgr: tauri::State<'_, Arc<VpnManager>>) -> Result<(), String> {
    // 注销前先断开,避免残留隧道。
    let _ = mgr.disconnect().await;
    let repo = repo()?;
    run_logout(&repo)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Whether a refresh token is stored locally (i.e. the user has logged in).
#[tauri::command]
pub async fn is_logged_in() -> bool {
    match repo() {
        Ok(r) => matches!(r.refresh_token(), Ok(Some(_))),
        Err(_) => false,
    }
}

/// The currently saved server URL, if any.
#[tauri::command]
pub async fn saved_server() -> Option<String> {
    repo().ok().and_then(|r| r.server_url().ok().flatten())
}
