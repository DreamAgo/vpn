//! Story 4.13 / 4.15: 命令行解析与控制命令。
//!
//! 用 `clap`(derive) 定义子命令：
//! - `login` / `logout`：认证与凭证持久化（Story 4.13）。
//! - `up`(`connect`) / `down`(`disconnect`) / `status`：经 IPC 控制 daemon（Story 4.15）。
//! - `daemon install|uninstall|start|stop|status`：转发给 vpn-platform DaemonRuntime。
//! - `daemon run`：daemon 主循环入口（由服务管理器拉起）。
//!
//! 命令解析是纯逻辑，覆盖单测；执行路径中需网络 / IPC / 设备的部分在运行期生效。

use clap::{Parser, Subcommand};

use crate::config::{default_device_name, CredentialRepo, DaemonConfig};
use crate::error::{CliError, CliResult};
use crate::ipc::{self, ConnState, IpcRequest, IpcResponse};

/// vpn-cli：VPN 客户端命令行。
#[derive(Debug, Parser)]
#[command(name = "vpn-cli", version, about = "VPN 客户端 CLI")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

/// 顶层子命令。
#[derive(Debug, Subcommand)]
pub enum Command {
    /// 登录到 VPN 服务端并保存凭证。
    Login {
        /// 服务端 URL，如 https://vpn.example.com
        #[arg(long)]
        server: String,
        /// 用户名（缺省则交互式询问）。
        #[arg(long)]
        username: Option<String>,
        /// 密码（不建议命令行传入；缺省则交互式安全读入）。
        #[arg(long)]
        password: Option<String>,
    },
    /// 注销并清除本地凭证。
    Logout,
    /// 建立 VPN 连接（别名 connect）。
    #[command(alias = "connect")]
    Up,
    /// 断开 VPN 连接（别名 disconnect）。
    #[command(alias = "disconnect")]
    Down,
    /// 查看当前连接状态。
    Status,
    /// daemon 服务管理 / 运行。
    Daemon {
        #[command(subcommand)]
        action: DaemonAction,
    },
}

/// daemon 子命令。
#[derive(Debug, Subcommand, PartialEq, Eq)]
pub enum DaemonAction {
    /// 安装为系统服务（systemd user / launchd / Windows Service）。
    Install,
    /// 卸载系统服务。
    Uninstall,
    /// 启动服务。
    Start,
    /// 停止服务。
    Stop,
    /// 查询服务状态。
    Status,
    /// 在前台运行 daemon 主循环（由服务管理器拉起，一般不手动调用）。
    Run,
}

/// 纯逻辑：把 up/down/status 子命令映射为 IPC 请求。
pub fn command_to_ipc(cmd: &Command) -> Option<IpcRequest> {
    match cmd {
        Command::Up => Some(IpcRequest::Connect),
        Command::Down => Some(IpcRequest::Disconnect),
        Command::Status => Some(IpcRequest::GetStatus),
        _ => None,
    }
}

/// 纯逻辑：渲染 IPC 响应为用户可读文本（便于单测，与 IO 解耦）。
pub fn render_response(resp: &IpcResponse) -> String {
    match resp {
        IpcResponse::Ok => "OK".to_string(),
        IpcResponse::Error { message } => format!("错误: {message}"),
        IpcResponse::Status(s) => {
            let mut out = format!("状态: {}", s.state.label());
            if let Some(ip) = &s.vpn_ip {
                out.push_str(&format!("\nVPN IP: {ip}"));
            }
            if s.state == ConnState::Connected {
                out.push_str(&format!("\n流量: ↓{} B / ↑{} B", s.bytes_rx, s.bytes_tx));
            }
            if let Some(err) = &s.last_error {
                out.push_str(&format!("\n最近错误: {err}"));
            }
            out
        }
    }
}

/// 解析命令行（薄封装，便于 main 调用与测试）。
pub fn parse() -> Cli {
    Cli::parse()
}

// === 执行（含 IO） ===

/// login 命令：调用 API 登录，保存 server_url + refresh_token 到凭证存储。
pub async fn run_login(
    server: &str,
    username: Option<&str>,
    password: Option<&str>,
    repo: &CredentialRepo,
) -> CliResult<String> {
    let username = match username {
        Some(u) => u.to_string(),
        None => prompt_line("用户名: ")?,
    };
    let password = match password {
        Some(p) => p.to_string(),
        None => prompt_password("密码: ")?,
    };

    let api = crate::api::ApiClient::new(server)?;
    let resp = api.login(&username, &password).await?;
    repo.save_login(server, &resp.refresh_token, Some(&username))?;

    let mut msg = format!("登录成功，凭证已保存（服务端 {server}）。");
    if resp.must_change_password {
        msg.push_str("\n注意: 账户要求首次登录后修改密码。");
    }
    Ok(msg)
}

/// logout 命令：尝试服务端吊销 refresh token，并清本地凭证。
pub async fn run_logout(repo: &CredentialRepo) -> CliResult<String> {
    let server = repo.server_url()?;
    let refresh = repo.refresh_token()?;
    if let (Some(server), Some(refresh)) = (server, refresh) {
        let api = crate::api::ApiClient::new(&server)?;
        api.set_refresh_token(refresh);
        // 服务端吊销失败不阻塞本地清理（best-effort）。
        let _ = api.logout().await;
    }
    repo.clear()?;
    Ok("已注销，本地凭证已清除。".to_string())
}

/// up/down/status：经 IPC 向 daemon 发指令。
pub async fn run_ipc_command(cmd: &Command, socket: &std::path::Path) -> CliResult<String> {
    let req = command_to_ipc(cmd).ok_or_else(|| CliError::Invalid("非 IPC 命令".to_string()))?;
    let resp = ipc::send_request(socket, &req).await?;
    Ok(render_response(&resp))
}

/// daemon 服务管理：转发给 vpn-platform DaemonRuntime。
pub fn run_daemon_admin(action: &DaemonAction) -> CliResult<String> {
    let runtime = vpn_platform::default_runtime()?;
    match action {
        DaemonAction::Install => {
            let exe = std::env::current_exe()?;
            runtime.install(&exe, &["daemon".to_string(), "run".to_string()])?;
            Ok("daemon 已安装。".to_string())
        }
        DaemonAction::Uninstall => {
            runtime.uninstall()?;
            Ok("daemon 已卸载。".to_string())
        }
        DaemonAction::Start => {
            runtime.start()?;
            Ok("daemon 已启动。".to_string())
        }
        DaemonAction::Stop => {
            runtime.stop()?;
            Ok("daemon 已停止。".to_string())
        }
        DaemonAction::Status => {
            let status = runtime.status()?;
            Ok(format!("daemon 状态: {status:?}"))
        }
        DaemonAction::Run => Err(CliError::Invalid(
            "`daemon run` 应由 main 直接驱动".to_string(),
        )),
    }
}

/// 装配 daemon 运行配置（供 `daemon run`）。
pub fn build_daemon_config(repo: &CredentialRepo) -> CliResult<DaemonConfig> {
    repo.to_daemon_config(default_device_name(), ipc::default_socket_path())
}

// === 交互式输入辅助 ===

fn prompt_line(prompt: &str) -> CliResult<String> {
    use std::io::Write;
    print!("{prompt}");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

fn prompt_password(prompt: &str) -> CliResult<String> {
    rpassword::prompt_password(prompt).map_err(|e| CliError::Other(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::StatusResponse;
    use clap::CommandFactory;

    #[test]
    fn clap_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parse_login_with_flags() {
        let cli = Cli::try_parse_from([
            "vpn-cli",
            "login",
            "--server",
            "https://vpn.example.com",
            "--username",
            "alice",
        ])
        .unwrap();
        match cli.command {
            Command::Login {
                server,
                username,
                password,
                ..
            } => {
                assert_eq!(server, "https://vpn.example.com");
                assert_eq!(username, Some("alice".to_string()));
                assert_eq!(password, None);
            }
            other => panic!("expected Login, got {other:?}"),
        }
    }

    #[test]
    fn login_requires_server() {
        assert!(Cli::try_parse_from(["vpn-cli", "login"]).is_err());
    }

    #[test]
    fn login_rejects_removed_route_flag() {
        assert!(Cli::try_parse_from([
            "vpn-cli",
            "login",
            "--server",
            "https://s",
            "--route",
            "192.168.10.0/24",
        ])
        .is_err());
    }

    #[test]
    fn connect_alias_maps_to_up() {
        let cli = Cli::try_parse_from(["vpn-cli", "connect"]).unwrap();
        assert!(matches!(cli.command, Command::Up));
        let cli = Cli::try_parse_from(["vpn-cli", "up"]).unwrap();
        assert!(matches!(cli.command, Command::Up));
    }

    #[test]
    fn disconnect_alias_maps_to_down() {
        let cli = Cli::try_parse_from(["vpn-cli", "disconnect"]).unwrap();
        assert!(matches!(cli.command, Command::Down));
        let cli = Cli::try_parse_from(["vpn-cli", "down"]).unwrap();
        assert!(matches!(cli.command, Command::Down));
    }

    #[test]
    fn daemon_subcommands_parse() {
        for (arg, expected) in [
            ("install", DaemonAction::Install),
            ("uninstall", DaemonAction::Uninstall),
            ("start", DaemonAction::Start),
            ("stop", DaemonAction::Stop),
            ("status", DaemonAction::Status),
            ("run", DaemonAction::Run),
        ] {
            let cli = Cli::try_parse_from(["vpn-cli", "daemon", arg]).unwrap();
            match cli.command {
                Command::Daemon { action } => assert_eq!(action, expected),
                other => panic!("expected Daemon, got {other:?}"),
            }
        }
    }

    #[test]
    fn unknown_command_errors() {
        assert!(Cli::try_parse_from(["vpn-cli", "frobnicate"]).is_err());
    }

    #[test]
    fn command_to_ipc_mapping() {
        assert_eq!(command_to_ipc(&Command::Up), Some(IpcRequest::Connect));
        assert_eq!(command_to_ipc(&Command::Down), Some(IpcRequest::Disconnect));
        assert_eq!(
            command_to_ipc(&Command::Status),
            Some(IpcRequest::GetStatus)
        );
        assert_eq!(command_to_ipc(&Command::Logout), None);
    }

    #[test]
    fn render_ok_and_error() {
        assert_eq!(render_response(&IpcResponse::Ok), "OK");
        assert_eq!(
            render_response(&IpcResponse::Error {
                message: "x".into()
            }),
            "错误: x"
        );
    }

    #[test]
    fn render_status_connected_includes_traffic() {
        let resp = IpcResponse::Status(StatusResponse {
            state: ConnState::Connected,
            vpn_ip: Some("10.8.0.5".into()),
            since: Some(1),
            bytes_rx: 100,
            bytes_tx: 200,
            last_error: None,
        });
        let s = render_response(&resp);
        assert!(s.contains("connected"));
        assert!(s.contains("10.8.0.5"));
        assert!(s.contains("100"));
        assert!(s.contains("200"));
    }

    #[test]
    fn render_status_disconnected_no_traffic_line() {
        let resp = IpcResponse::Status(StatusResponse::disconnected());
        let s = render_response(&resp);
        assert!(s.contains("disconnected"));
        assert!(!s.contains("流量"));
    }

    #[test]
    fn render_status_with_error() {
        let mut snap = StatusResponse::disconnected();
        snap.state = ConnState::Error;
        snap.last_error = Some("连接被拒绝".into());
        let s = render_response(&IpcResponse::Status(snap));
        assert!(s.contains("error"));
        assert!(s.contains("连接被拒绝"));
    }
}
