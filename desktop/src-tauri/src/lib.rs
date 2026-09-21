//! VPN Client desktop GUI (Tauri 2) — Dock + menu-bar/tray app for macOS.
//!
//! 以 `Regular` 激活策略运行：既在程序坞显示图标，也在菜单栏放一个托盘图标。
//! 点托盘图标切换小型无边框弹出窗口；关闭按钮只隐藏窗口（保活），点程序坞图标
//! （`RunEvent::Reopen`）或托盘可重新唤出。托盘右键菜单提供 Open / Connect /
//! Disconnect / Quit。所有 VPN 工作进程内完成（库调用 `vpn-cli`），见 `manager.rs`。

mod commands;
#[cfg(target_os = "macos")]
mod macos_helper;
mod manager;
mod observability;
mod updates;

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use manager::VpnManager;
#[cfg(target_os = "macos")]
use tauri::menu::{PredefinedMenuItem, Submenu};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent},
    Manager, WindowEvent,
};

struct TrayUi {
    tray: TrayIcon<tauri::Wry>,
    connect: MenuItem<tauri::Wry>,
    disconnect: MenuItem<tauri::Wry>,
}

fn should_keep_window_alive(label: &str) -> bool {
    label == "main"
}

#[derive(Default)]
struct ExitState {
    pending: AtomicBool,
    ready: AtomicBool,
    connect: tokio::sync::Mutex<()>,
}

pub(crate) async fn connect_vpn(app: &tauri::AppHandle) -> Result<(), String> {
    let state = app.state::<ExitState>();
    let _guard = state.connect.lock().await;
    if state.pending.load(Ordering::Acquire) {
        return Err("正在退出易链，请稍后再连接".to_string());
    }
    #[cfg(target_os = "macos")]
    {
        macos_helper::connect().await
    }
    #[cfg(not(target_os = "macos"))]
    {
        app.state::<Arc<VpnManager>>().connect().await
    }
}

fn disconnect_and_exit(app: &tauri::AppHandle, code: i32) {
    if app
        .state::<ExitState>()
        .pending
        .swap(true, Ordering::AcqRel)
    {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<ExitState>();
        // 等待包含 helper 安装在内的建连请求结束，防止断开后迟到建连。
        let _guard = state.connect.lock().await;
        #[cfg(target_os = "macos")]
        let result = macos_helper::disconnect_before_exit().await;
        #[cfg(not(target_os = "macos"))]
        let result = app.state::<Arc<VpnManager>>().disconnect().await;
        match result {
            Ok(()) => {
                state.ready.store(true, Ordering::Release);
                app.exit(code);
            }
            Err(error) => {
                use tauri_plugin_notification::NotificationExt;
                let error = vpn_cli::error::redact_sensitive(&error);
                tracing::error!(%error, "退出前断开 VPN 失败，保留客户端以便重试");
                state.pending.store(false, Ordering::Release);
                show_window(&app);
                let _ = app
                    .notification()
                    .builder()
                    .title("退出失败")
                    .body(format!("无法确认 VPN 已断开，请重试：{error}"))
                    .show();
            }
        }
    });
}

/// Show + focus the main popover window(健壮版:取消最小化 + 置顶一次 + 聚焦)。
pub(crate) fn show_window(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.unminimize();
        let _ = win.show();
        let _ = win.set_focus();
    }
}

/// Hide the main popover window. Exposed as a command so JS can request a hide.
#[tauri::command]
fn hide_window(window: tauri::Window) {
    let _ = window.hide();
}

/// 请求退出整个 App；ExitRequested 统一负责先断开 VPN。
#[tauri::command]
fn quit_app(app: tauri::AppHandle) {
    app.exit(0);
}

#[tauri::command]
fn sync_tray_state(app: tauri::AppHandle, state: String) {
    if let Some(ui) = app.try_state::<TrayUi>() {
        let connected = state == "connected" || state == "reconnecting";
        let connecting = state == "connecting";
        let errored = state == "error";
        let _ = ui.connect.set_enabled(!connected && !connecting);
        let _ = ui.disconnect.set_enabled(connected || connecting);
        let label = match state.as_str() {
            "connected" => "已连接",
            "connecting" => "连接中",
            "reconnecting" => "重连中",
            "error" => "连接异常",
            _ => "未连接",
        };
        let suffix = if errored {
            " · 请查看错误详情"
        } else {
            ""
        };
        let _ = ui.tray.set_tooltip(Some(format!("易链 · {label}{suffix}")));
    }
}

/// 从托盘菜单触发连接(进程内库调用,fire-and-forget;结果反映在状态轮询里)。
fn spawn_connect(app: &tauri::AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = connect_vpn(&app).await {
            tracing::warn!(error = %vpn_cli::error::redact_sensitive(&error), "托盘连接操作失败");
        }
    });
}

/// 从托盘菜单触发断开。
fn spawn_disconnect(app: &tauri::AppHandle) {
    #[cfg(target_os = "macos")]
    {
        let _ = app;
        tauri::async_runtime::spawn(async move {
            if let Err(error) = macos_helper::disconnect().await {
                tracing::warn!(error = %vpn_cli::error::redact_sensitive(&error), "托盘断开操作失败");
            }
        });
    }
    #[cfg(not(target_os = "macos"))]
    {
        let mgr = app.state::<Arc<VpnManager>>().inner().clone();
        tauri::async_runtime::spawn(async move {
            if let Err(error) = mgr.disconnect().await {
                let error = vpn_cli::error::redact_sensitive(&error);
                tracing::warn!(%error, "托盘断开操作失败");
            }
        });
    }
}

#[cfg(target_os = "macos")]
pub fn run_macos_helper_if_requested() -> Option<i32> {
    if macos_helper::print_build_hash_requested() {
        return Some(match macos_helper::print_build_hash() {
            Ok(()) => 0,
            Err(error) => {
                eprintln!("vpn-desktop: 无法计算构建摘要: {error}");
                1
            }
        });
    }
    if !macos_helper::helper_mode_requested() {
        return None;
    }
    Some(match macos_helper::run_helper_from_args() {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("vpn-desktop helper: {error}");
            1
        }
    })
}

#[cfg(not(target_os = "macos"))]
pub fn run_macos_helper_if_requested() -> Option<i32> {
    None
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(target_os = "macos")]
    if let Err(error) = macos_helper::initialize_gui_build_hash() {
        eprintln!("vpn-desktop: 无法固定 helper 构建摘要: {error}");
    }
    // This must be the first network-related initialization in the desktop
    // process. Tauri's updater also uses rustls and otherwise may select ring
    // before vpn-cli can install the AWS-LC provider required on networks that
    // accept only the post-quantum ClientHello used by the working client.
    vpn_cli::api::install_tls_crypto_provider()
        .expect("failed to initialize the required TLS crypto provider");
    let _diagnostics = observability::init();

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![]),
        ))
        .manage(Arc::new(VpnManager::new()))
        .manage(ExitState::default())
        .invoke_handler(tauri::generate_handler![
            updates::update_source,
            updates::check_server_update,
            commands::get_status,
            commands::connect,
            commands::disconnect,
            commands::login,
            commands::feishu_login_available,
            commands::feishu_login,
            commands::logout,
            commands::change_password,
            commands::is_logged_in,
            commands::saved_server,
            commands::saved_username,
            commands::diagnostics_info,
            commands::read_recent_logs,
            hide_window,
            quit_app,
            sync_tray_state,
        ])
        .setup(|app| {
            // Windows GUI 以管理员身份运行；在页面登录请求之前恢复上次遗留 DNS。
            #[cfg(target_os = "windows")]
            tauri::async_runtime::block_on(vpn_cli::daemon::cleanup_dns_before_connect())?;

            // Windows:把随包分发的 wintun.dll 绝对路径告诉数据面(vpn-cli 据此显式 load),
            // 避免依赖工作目录搜索 DLL。dev / 未打包场景两处候选都不存在时,回退 tun 默认
            // 的 "wintun.dll" 搜索(由 wg_userspace.rs 处理 VPN_WINTUN_PATH 未设置的情况)。
            #[cfg(target_os = "windows")]
            if let Ok(res) = app.path().resource_dir() {
                for cand in [
                    res.join("wintun.dll"),
                    res.join("resources").join("wintun.dll"),
                ] {
                    if cand.exists() {
                        std::env::set_var("VPN_WINTUN_PATH", &cand);
                        break;
                    }
                }
            }

            // Regular 策略：程序坞显示图标 + 拥有菜单栏（Edit 菜单 Cmd+V 才稳）。
            // 托盘图标与之并存（托盘独立于激活策略）。
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Regular);

            // macOS accessory 应用默认不带「编辑」菜单，导致输入框 Cmd+C/V/X/A 全部失效
            // （没有菜单项提供这些 key equivalent，WKWebView 收不到标准编辑命令）。
            // 显式装一个标准 Edit 菜单即可把粘贴/复制/剪切/全选接回响应链。
            #[cfg(target_os = "macos")]
            {
                let edit = Submenu::with_items(
                    app,
                    "Edit",
                    true,
                    &[
                        &PredefinedMenuItem::undo(app, None)?,
                        &PredefinedMenuItem::redo(app, None)?,
                        &PredefinedMenuItem::separator(app)?,
                        &PredefinedMenuItem::cut(app, None)?,
                        &PredefinedMenuItem::copy(app, None)?,
                        &PredefinedMenuItem::paste(app, None)?,
                        &PredefinedMenuItem::select_all(app, None)?,
                    ],
                )?;
                let app_menu = Menu::with_items(app, &[&edit])?;
                app.set_menu(app_menu)?;
            }

            // Tray context menu.
            let open_i = MenuItem::with_id(app, "open", "打开易链", true, None::<&str>)?;
            let connect_i = MenuItem::with_id(app, "connect", "建立安全链路", true, None::<&str>)?;
            let disconnect_i =
                MenuItem::with_id(app, "disconnect", "断开连接", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "退出易链", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open_i, &connect_i, &disconnect_i, &quit_i])?;

            // 专门的小尺寸单色托盘图标。macOS template 模式会根据菜单栏明暗
            // 自动渲染为黑色或白色，符合状态栏图标规范。
            let tray_icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?;

            let tray = TrayIconBuilder::with_id("main-tray")
                .icon(tray_icon)
                .icon_as_template(true)
                .tooltip("易链")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => show_window(app),
                    "connect" => spawn_connect(app),
                    "disconnect" => spawn_disconnect(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    // Left click toggles the popover window.
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(win) = app.get_webview_window("main") {
                            if win.is_visible().unwrap_or(false) {
                                let _ = win.hide();
                            } else {
                                show_window(app);
                            }
                        }
                    }
                })
                .build(app)?;

            let _ = disconnect_i.set_enabled(false);
            app.manage(TrayUi {
                tray,
                connect: connect_i.clone(),
                disconnect: disconnect_i.clone(),
            });

            // 首次启动直接弹出面板，便于发现 UI（菜单栏 App 默认隐藏窗口，
            // 否则用户只能靠右上角托盘图标唤出，容易找不到）。失焦后会自动隐藏，
            // 之后点托盘图标再唤出。
            show_window(app.handle());

            Ok(())
        })
        .on_window_event(|window, event| {
            // 主窗口关闭按钮只隐藏不销毁（保活在菜单栏）；授权等临时窗口必须允许
            // 真正销毁，否则其固定 label 无法在下一次流程中复用。
            if let WindowEvent::CloseRequested { api, .. } = event {
                if should_keep_window_alive(window.label()) {
                    let _ = window.hide();
                    api.prevent_close();
                }
            }
        })
        .build(tauri::generate_context!());

    let app = match app {
        Ok(app) => app,
        Err(error) => {
            tracing::error!(%error, "构建 Tauri 应用失败");
            eprintln!("vpn-desktop: 构建应用失败: {error}");
            return;
        }
    };

    app.run(|_app_handle, _event| {
        if let tauri::RunEvent::ExitRequested { api, code, .. } = &_event {
            // Tauri 的更新重启不能被 prevent_exit 拦截，保持其原有行为。
            if *code != Some(tauri::RESTART_EXIT_CODE)
                && !_app_handle
                    .state::<ExitState>()
                    .ready
                    .load(Ordering::Acquire)
            {
                api.prevent_exit();
                disconnect_and_exit(_app_handle, code.unwrap_or(0));
            }
        }
        // 点击程序坞图标时（窗口可能已隐藏）重新唤出窗口。
        // RunEvent::Reopen 仅 macOS 存在（dock 点击），其它平台无此变体，需 cfg 隔离。
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Reopen { .. } = _event {
            show_window(_app_handle);
        }
        if matches!(_event, tauri::RunEvent::Exit) {
            tracing::info!("桌面客户端正常退出");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::should_keep_window_alive;

    #[test]
    fn main_window_is_kept_alive_on_close() {
        assert!(should_keep_window_alive("main"));
    }

    #[test]
    fn temporary_auth_window_is_allowed_to_close() {
        assert!(!should_keep_window_alive("feishu-auth"));
    }
}
