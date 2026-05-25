//! vpn-cli 可执行入口。
//!
//! 解析命令行并分发到 [`vpn_cli::cli`] 中的执行函数。需要网络 / IPC / 设备 /
//! 钥匙串的路径在运行期生效；无凭证或 daemon 未运行时给出可读提示。

use vpn_cli::cli::{self, Command, DaemonAction};
use vpn_cli::config::CredentialRepo;
use vpn_cli::error::CliResult;
use vpn_cli::{daemon, ipc};

#[tokio::main]
async fn main() {
    init_tracing();
    if let Err(e) = run().await {
        eprintln!("vpn-cli: {e}");
        std::process::exit(1);
    }
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}

async fn run() -> CliResult<()> {
    let cli = cli::parse();
    // 凭证后端：优先 keyring，失败降级加密文件。
    let repo = credential_repo();

    match &cli.command {
        Command::Login {
            server,
            username,
            password,
        } => {
            let msg =
                cli::run_login(server, username.as_deref(), password.as_deref(), &repo).await?;
            println!("{msg}");
        }
        Command::Logout => {
            let msg = cli::run_logout(&repo).await?;
            println!("{msg}");
        }
        Command::Up | Command::Down | Command::Status => {
            let socket = ipc::default_socket_path();
            let msg = cli::run_ipc_command(&cli.command, &socket).await?;
            println!("{msg}");
        }
        Command::Daemon { action } => match action {
            DaemonAction::Run => {
                let config = cli::build_daemon_config(&repo)?;
                tracing::info!(server = %config.server_url, "daemon 启动");
                daemon::run(config).await?;
            }
            other => {
                let msg = cli::run_daemon_admin(other)?;
                println!("{msg}");
            }
        },
    }
    Ok(())
}

/// 选择凭证后端：系统钥匙串（keyring）为主路径。
///
/// 真机降级：当 keyring 不可用（如 Linux headless 无 libsecret）时，可改用
/// [`CredentialRepo::file`]；本入口默认 keyring，降级策略留给部署配置。
fn credential_repo() -> CredentialRepo {
    CredentialRepo::keyring()
}
