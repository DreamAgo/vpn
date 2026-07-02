//! VPN Client desktop GUI (Tauri 2) — Dock + menu-bar/tray app for macOS.
//!
//! 以 `Regular` 激活策略运行：既在程序坞显示图标，也在菜单栏放一个托盘图标。
//! 点托盘图标切换小型无边框弹出窗口；关闭按钮只隐藏窗口（保活），点程序坞图标
//! （`RunEvent::Reopen`）或托盘可重新唤出。托盘右键菜单提供 Open / Connect /
//! Disconnect / Quit。所有 VPN 工作进程内完成（库调用 `vpn-cli`），见 `manager.rs`。

mod commands;
mod manager;

use std::sync::Arc;

use manager::VpnManager;
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent},
    Manager, WindowEvent,
};

struct TrayUi {
    tray: TrayIcon<tauri::Wry>,
    connect: MenuItem<tauri::Wry>,
    disconnect: MenuItem<tauri::Wry>,
}

/// Show + focus the main popover window(健壮版:取消最小化 + 置顶一次 + 聚焦)。
fn show_window(app: &tauri::AppHandle) {
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

/// 退出整个 App(窗口内"退出"按钮调用)。菜单栏 App 无程序坞图标,这是保底退出入口。
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
        let suffix = if errored { " · 请查看错误详情" } else { "" };
        let _ = ui.tray.set_tooltip(Some(format!("易链 · {label}{suffix}")));
    }
}

/// 从托盘菜单触发连接(进程内库调用,fire-and-forget;结果反映在状态轮询里)。
fn spawn_connect(app: &tauri::AppHandle) {
    let mgr = app.state::<Arc<VpnManager>>().inner().clone();
    tauri::async_runtime::spawn(async move {
        let _ = mgr.connect().await;
    });
}

/// 从托盘菜单触发断开。
fn spawn_disconnect(app: &tauri::AppHandle) {
    let mgr = app.state::<Arc<VpnManager>>().inner().clone();
    tauri::async_runtime::spawn(async move {
        let _ = mgr.disconnect().await;
    });
}

/// macOS release 构建:若当前不是 root,弹系统管理员密码框、以 root 重启自己,
/// 然后退出本(非 root)实例 —— 这样**双击图标即可获得 root**(开 TUN 所需),
/// 无需终端 `sudo`。
///
/// 仅在 release 生效;dev(debug)构建跳过,方便纯 UI 调试(此时 Connect 因无
/// root 会在面板报错而非崩溃)。设环境变量 `VPN_DESKTOP_NO_ELEVATE=1` 也可跳过。
#[cfg(all(target_os = "macos", not(debug_assertions)))]
fn maybe_elevate() {
    // SAFETY: geteuid 无副作用、始终安全。
    if unsafe { libc::geteuid() } == 0 {
        return; // 已是 root,继续启动。
    }
    if std::env::var_os("VPN_DESKTOP_NO_ELEVATE").is_some() {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    // 当前(非 root)实例的真实用户 uid —— 用它把提权后的 root 实例拉回该用户的 GUI 会话。
    // SAFETY: getuid 无副作用、始终安全。
    let uid = unsafe { libc::getuid() };
    let exe = exe.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"");
    // 经 osascript 弹管理员密码框,以 root 启动自己。**关键**:用 `launchctl asuser <uid>`
    // 在该用户的 GUI(Aqua)会话上下文里启动——否则经 osascript 提权的 root 进程会脱离用户的
    // WindowServer 会话,菜单栏/程序坞图标都不显示、编辑菜单 Cmd+V 也失效。asuser 只改 Mach
    // bootstrap 会话、不降权(仍是 root,可开 TUN)。尾部 & 让脚本立即返回。
    let script = format!(
        "do shell script \"/bin/launchctl asuser {uid} \\\"{exe}\\\" >/dev/null 2>&1 &\" with administrator privileges"
    );
    let elevated = std::process::Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if elevated {
        // root 实例已拉起,本(非 root)实例退出,避免双实例窗口。
        std::process::exit(0);
    }
    // 提权被取消或失败(如用户点了「取消」、osascript 出错):**不退出**。保留当前非 root
    // 实例继续运行,让用户至少看到界面;后续 Connect 会在面板内报权限错误,而不是
    // 整个应用静默消失、零反馈(原先无条件 exit(0) 会导致取消提权后应用「打不开」)。
    eprintln!(
        "[vpn-desktop] 管理员提权未完成,以非特权模式继续运行;连接将因缺少权限而失败"
    );
}

/// 其它平台 / dev 构建:不在此处运行时自提权。
/// - Windows:由 build.rs 注入的 `requireAdministrator` 清单在进程启动时弹 UAC 提权;
/// - Linux:暂未实现自提权,需以 root 运行(或后续接 pkexec / 特权 helper,见 README)。
#[cfg(not(all(target_os = "macos", not(debug_assertions))))]
fn maybe_elevate() {}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 必须在创建任何窗口/事件循环之前完成提权(否则会出现两个实例的窗口)。
    maybe_elevate();

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![]),
        ))
        .manage(Arc::new(VpnManager::new()))
        .invoke_handler(tauri::generate_handler![
            commands::get_status,
            commands::connect,
            commands::disconnect,
            commands::login,
            commands::logout,
            commands::change_password,
            commands::is_logged_in,
            commands::saved_server,
            hide_window,
            quit_app,
            sync_tray_state,
        ])
        .setup(|app| {
            // Windows:把随包分发的 wintun.dll 绝对路径告诉数据面(vpn-cli 据此显式 load),
            // 避免依赖工作目录搜索 DLL。dev / 未打包场景两处候选都不存在时,回退 tun 默认
            // 的 "wintun.dll" 搜索(由 wg_userspace.rs 处理 VPN_WINTUN_PATH 未设置的情况)。
            #[cfg(target_os = "windows")]
            if let Ok(res) = app.path().resource_dir() {
                for cand in [res.join("wintun.dll"), res.join("resources").join("wintun.dll")] {
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

            // 专门的小尺寸单色托盘图标(44×44,带 alpha);macOS 以 template 模式
            // 渲染,随菜单栏明暗主题自动着色,避免用 512×512 app 图标缩放后看不清/不显示。
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
            // 关闭按钮只隐藏不销毁(保活在菜单栏)。**不再**失焦自动隐藏——
            // macOS 上隐藏后常唤不回来,导致"失焦就再也打不开";改由托盘点击切换 /
            // 关闭按钮显隐。
            if let WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app_handle, _event| {
            // 点击程序坞图标时（窗口可能已隐藏）重新唤出窗口。
            // RunEvent::Reopen 仅 macOS 存在（dock 点击），其它平台无此变体，需 cfg 隔离。
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = _event {
                show_window(_app_handle);
            }
        });
}
