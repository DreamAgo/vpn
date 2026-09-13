//! Tauri commands: thin bridge from the React front-end to the in-process VPN
//! connection manager ([`crate::manager::VpnManager`]) and the credential store.
//!
//! 单进程架构:`connect`/`disconnect`/`get_status` 直接驱动本进程内的用户态
//! 隧道(库调用 `vpn-cli`),不经独立 daemon / IPC。`login`/`logout` 仍走凭证存储。

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

use tauri::Manager;
use vpn_api_types::auth::FeishuAuthPollStatus;
use vpn_cli::api::ApiClient;
use vpn_cli::cli::{run_login, run_logout};
use vpn_cli::config::CredentialRepo;
use vpn_cli::ipc::StatusResponse;

use crate::manager::VpnManager;
use crate::observability::{self, DiagnosticsInfo, LogSnapshot};

static FEISHU_LOGIN_IN_FLIGHT: AtomicBool = AtomicBool::new(false);
const FEISHU_AUTH_WINDOW_LABEL: &str = "feishu-auth";
const FEISHU_AUTH_WINDOW_CLOSE_POLL_ATTEMPTS: usize = 40;
const FEISHU_AUTH_WINDOW_CLOSE_POLL_INTERVAL: Duration = Duration::from_millis(50);

struct FeishuLoginGuard;

impl FeishuLoginGuard {
    fn acquire() -> Result<Self, String> {
        FEISHU_LOGIN_IN_FLIGHT
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| Self)
            .map_err(|_| "已有飞书登录正在进行，请在浏览器中完成或等待超时".to_string())
    }
}

impl Drop for FeishuLoginGuard {
    fn drop(&mut self) {
        FEISHU_LOGIN_IN_FLIGHT.store(false, Ordering::Release);
    }
}

struct FeishuAuthWindowGuard {
    window: tauri::WebviewWindow,
}

impl FeishuAuthWindowGuard {
    fn close(&self) {
        let _ = self.window.close();
    }
}

impl Drop for FeishuAuthWindowGuard {
    fn drop(&mut self) {
        self.close();
    }
}

async fn wait_until<F>(mut condition: F, attempts: usize, interval: Duration) -> bool
where
    F: FnMut() -> bool,
{
    for attempt in 0..attempts {
        if condition() {
            return true;
        }
        if attempt + 1 < attempts {
            tokio::time::sleep(interval).await;
        }
    }
    false
}

async fn close_existing_feishu_auth_window(app: &tauri::AppHandle) -> Result<(), String> {
    let Some(stale) = app.get_webview_window(FEISHU_AUTH_WINDOW_LABEL) else {
        return Ok(());
    };
    stale
        .close()
        .map_err(|error| format!("无法关闭已有飞书授权窗口：{error}"))?;
    drop(stale);

    if wait_until(
        || app.get_webview_window(FEISHU_AUTH_WINDOW_LABEL).is_none(),
        FEISHU_AUTH_WINDOW_CLOSE_POLL_ATTEMPTS,
        FEISHU_AUTH_WINDOW_CLOSE_POLL_INTERVAL,
    )
    .await
    {
        Ok(())
    } else {
        Err("旧飞书授权窗口未能关闭，请手动关闭后重试".to_string())
    }
}

fn ensure_feishu_auth_window_open(window_exists: bool) -> Result<(), String> {
    if window_exists {
        Ok(())
    } else {
        Err("飞书授权已取消，请重试".to_string())
    }
}

/// Open a file-backed credential repo (most reliable, no keyring prompts).
fn repo() -> Result<CredentialRepo, String> {
    CredentialRepo::file().map_err(|e| e.to_string())
}

/// 当前连接状态(前端每 2.5s 轮询)。读本进程内状态,不会失败。
#[tauri::command]
pub async fn get_status(mgr: tauri::State<'_, Arc<VpnManager>>) -> Result<StatusResponse, ()> {
    #[cfg(target_os = "macos")]
    {
        let _ = mgr;
        Ok(crate::macos_helper::status().await)
    }
    #[cfg(not(target_os = "macos"))]
    Ok(mgr.status().await)
}

/// 建立连接(注册 + 建用户态隧道 + 心跳)。需以特权运行(开 TUN)。
#[tauri::command]
pub async fn connect(mgr: tauri::State<'_, Arc<VpnManager>>) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let _ = mgr;
        crate::macos_helper::connect().await
    }
    #[cfg(not(target_os = "macos"))]
    mgr.connect().await
}

/// 断开连接。
#[tauri::command]
pub async fn disconnect(mgr: tauri::State<'_, Arc<VpnManager>>) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let _ = mgr;
        crate::macos_helper::disconnect().await
    }
    #[cfg(not(target_os = "macos"))]
    mgr.disconnect().await
}

/// Log in and persist credentials. Returns Err(message) on failure.
#[tauri::command]
pub async fn login(server: String, username: String, password: String) -> Result<(), String> {
    let repo = repo()?;
    run_login(&server, Some(&username), Some(&password), &repo)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn feishu_login_available(server: String) -> Result<bool, String> {
    let server = validate_server_url(&server)?;
    let api = ApiClient::new(server).map_err(|e| e.to_string())?;
    api.feishu_config()
        .await
        .map(|config| config.enabled)
        .map_err(|e| e.to_string())
}

/// 发起飞书授权，在独立受控 WebView 中完成扫码，并限时短轮询。
#[tauri::command]
pub async fn feishu_login(app: tauri::AppHandle, server: String) -> Result<(), String> {
    let _single_flight = FeishuLoginGuard::acquire()?;
    let server = validate_server_url(&server)?;
    let api = ApiClient::new(&server).map_err(|e| e.to_string())?;
    close_existing_feishu_auth_window(&app).await?;
    let started = api.feishu_start().await.map_err(|e| e.to_string())?;
    validate_authorization_url(&started.authorization_url)?;
    let authorization_url = url::Url::parse(&started.authorization_url)
        .map_err(|_| "服务端返回了无效的飞书授权地址".to_string())?;
    let mut auth_window_builder = tauri::WebviewWindowBuilder::new(
        &app,
        FEISHU_AUTH_WINDOW_LABEL,
        tauri::WebviewUrl::External(authorization_url),
    )
    .title("飞书授权")
    .inner_size(520.0, 720.0)
    .center()
    .focused(true)
    .always_on_top(true)
    .on_navigation(|url| url.scheme() == "https");
    if let Some(main_window) = app.get_webview_window("main") {
        auth_window_builder = auth_window_builder
            .parent(&main_window)
            .map_err(|error| format!("无法绑定飞书授权窗口：{error}"))?;
    }
    let auth_window = auth_window_builder
        .build()
        .map_err(|error| format!("无法打开飞书授权窗口：{error}"))?;
    let auth_window = FeishuAuthWindowGuard {
        window: auth_window,
    };
    let deadline = tokio::time::Instant::now()
        + std::time::Duration::from_secs(started.expires_in.max(1) as u64);
    loop {
        ensure_feishu_auth_window_open(app.get_webview_window(FEISHU_AUTH_WINDOW_LABEL).is_some())?;
        if tokio::time::Instant::now() >= deadline {
            return Err("飞书登录已超时，请重试".to_string());
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        tokio::time::sleep(remaining.min(std::time::Duration::from_secs(2))).await;
        ensure_feishu_auth_window_open(app.get_webview_window(FEISHU_AUTH_WINDOW_LABEL).is_some())?;
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err("飞书登录已超时，请重试".to_string());
        }
        let response = tokio::time::timeout(remaining, api.feishu_poll(&started.poll_token))
            .await
            .map_err(|_| "飞书登录已超时，请重试".to_string())?
            .map_err(|e| e.to_string())?;
        if let Err(error) = ensure_feishu_auth_window_open(
            app.get_webview_window(FEISHU_AUTH_WINDOW_LABEL).is_some(),
        ) {
            if matches!(response.status, FeishuAuthPollStatus::Complete) {
                let _ = api.logout().await;
            }
            return Err(error);
        }
        if matches!(response.status, FeishuAuthPollStatus::Complete) {
            let Some(username) = response.username else {
                let _ = api.logout().await;
                return Err("飞书登录响应缺少用户名".to_string());
            };
            let Some(login) = response.login else {
                let _ = api.logout().await;
                return Err("飞书登录响应缺少凭证".to_string());
            };
            let save_result = repo().and_then(|credentials| {
                credentials
                    .save_login(&server, &login.refresh_token, Some(&username))
                    .map_err(|error| error.to_string())
            });
            if let Err(error) = save_result {
                // 本地持久化失败时撤销刚签发的远端会话，避免遗留孤儿 refresh token。
                let _ = api.logout().await;
                return Err(error);
            }
            drop(auth_window);
            crate::show_window(&app);
            return Ok(());
        }
    }
}

fn validate_server_url(input: &str) -> Result<String, String> {
    let value = input.trim();
    let url = url::Url::parse(value).map_err(|_| "服务端地址无效".to_string())?;
    let host = url
        .host_str()
        .ok_or_else(|| "服务端地址缺少主机名".to_string())?;
    let local = matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]");
    if url.scheme() != "https" && !(local && url.scheme() == "http") {
        return Err(
            "飞书登录要求 HTTPS 服务端（localhost/loopback 开发环境可使用 HTTP）".to_string(),
        );
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("服务端地址不得包含用户凭证".to_string());
    }
    Ok(value.trim_end_matches('/').to_string())
}

fn validate_authorization_url(input: &str) -> Result<(), String> {
    let url = url::Url::parse(input).map_err(|_| "服务端返回了无效的飞书授权地址".to_string())?;
    if url.scheme() != "https" || url.host_str() != Some("accounts.feishu.cn") {
        return Err("服务端返回的飞书授权地址不可信".to_string());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("服务端返回的飞书授权地址不可信".to_string());
    }
    Ok(())
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
    #[cfg(target_os = "macos")]
    {
        let _ = mgr;
        crate::macos_helper::disconnect_before_logout().await?;
    }
    #[cfg(not(target_os = "macos"))]
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

/// The currently saved login username, if an active local session and the
/// optional username key both exist.
#[tauri::command]
pub async fn saved_username() -> Result<Option<String>, String> {
    let repo = repo()?;
    if repo.refresh_token().map_err(|e| e.to_string())?.is_none() {
        return Ok(None);
    }
    repo.username().map_err(|e| e.to_string())
}

/// 返回不含凭证与密钥的本地诊断元数据。
#[tauri::command]
pub fn diagnostics_info() -> DiagnosticsInfo {
    observability::diagnostics()
}

/// 返回固定日志目录中有界且二次脱敏的近期日志。
#[tauri::command]
pub async fn read_recent_logs() -> Result<LogSnapshot, String> {
    let desktop = tauri::async_runtime::spawn_blocking(observability::recent_logs)
        .await
        .map_err(|error| format!("读取日志任务失败: {error}"))??;
    #[cfg(target_os = "macos")]
    {
        let helper = crate::macos_helper::logs().await.unwrap_or_else(|error| {
            vpn_cli::ipc::HelperLogSnapshot {
                content: format!(
                    "helper 日志不可用: {}",
                    vpn_cli::error::redact_sensitive(&error)
                ),
                line_count: 1,
                truncated: false,
            }
        });
        Ok(observability::merge_helper_logs(helper, desktop))
    }
    #[cfg(not(target_os = "macos"))]
    Ok(desktop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_url_requires_https_except_loopback() {
        assert!(validate_server_url("https://vpn.example.com/").is_ok());
        assert!(validate_server_url("http://localhost:8080").is_ok());
        assert!(validate_server_url("http://127.0.0.1:8080").is_ok());
        assert!(validate_server_url("http://[::1]:8080").is_ok());
        assert!(validate_server_url("http://vpn.example.com").is_err());
        assert!(validate_server_url("https://user:pw@vpn.example.com").is_err());
    }

    #[test]
    fn authorization_url_is_pinned_to_feishu_https_host() {
        assert!(validate_authorization_url(
            "https://accounts.feishu.cn/open-apis/authen/v1/authorize?state=x"
        )
        .is_ok());
        assert!(validate_authorization_url("http://accounts.feishu.cn/path").is_err());
        assert!(validate_authorization_url("https://accounts.feishu.cn.evil.test/path").is_err());
        assert!(validate_authorization_url("https://user@accounts.feishu.cn/path").is_err());
    }

    #[test]
    fn feishu_login_guard_is_single_flight() {
        FEISHU_LOGIN_IN_FLIGHT.store(false, Ordering::Release);
        let first = FeishuLoginGuard::acquire().unwrap();
        assert!(FeishuLoginGuard::acquire().is_err());
        drop(first);
        assert!(FeishuLoginGuard::acquire().is_ok());
    }

    #[tokio::test]
    async fn wait_until_returns_immediately_when_condition_is_ready() {
        let polls = std::sync::atomic::AtomicUsize::new(0);

        assert!(
            wait_until(
                || {
                    polls.fetch_add(1, Ordering::Relaxed);
                    true
                },
                4,
                Duration::ZERO,
            )
            .await
        );
        assert_eq!(polls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn wait_until_handles_delayed_release() {
        let polls = std::sync::atomic::AtomicUsize::new(0);

        assert!(
            wait_until(
                || polls.fetch_add(1, Ordering::Relaxed) >= 2,
                4,
                Duration::ZERO,
            )
            .await
        );
        assert_eq!(polls.load(Ordering::Relaxed), 3);
    }

    #[tokio::test]
    async fn wait_until_stops_at_attempt_limit() {
        let polls = std::sync::atomic::AtomicUsize::new(0);

        assert!(
            !wait_until(
                || {
                    polls.fetch_add(1, Ordering::Relaxed);
                    false
                },
                4,
                Duration::ZERO,
            )
            .await
        );
        assert_eq!(polls.load(Ordering::Relaxed), 4);
    }

    #[test]
    fn closed_auth_window_is_treated_as_user_cancellation() {
        assert!(ensure_feishu_auth_window_open(true).is_ok());
        assert_eq!(
            ensure_feishu_auth_window_open(false).unwrap_err(),
            "飞书授权已取消，请重试"
        );
    }
}
