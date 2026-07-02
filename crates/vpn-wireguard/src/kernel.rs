//! 内核 WireGuard 控制平面实现（Linux）。
//!
//! 通过 `ip` 与 `wg` 命令操作内核 WireGuard 接口，实现 [`WireGuardControl`]。
//! 选用内核态而非 boringtun 用户态的原因：
//! - 服务端在 Linux 上内核 WireGuard 性能最佳、最稳定；
//! - 彻底避开 `boringtun 0.6` 与 `x25519-dalek` 的版本冲突（不引入 boringtun）。
//!
//! 运行要求：Linux + 内核 WireGuard 支持 + 已安装 `wireguard-tools`（`wg`）+
//! `CAP_NET_ADMIN`（通常以 root 运行）。
//!
//! 密钥处理：服务端私钥写入权限 0600 的临时文件后交给 `wg set private-key <file>`，
//! 避免出现在进程命令行参数中；用完即删。

use std::net::Ipv4Addr;

use async_trait::async_trait;
use vpn_core::{AppError, Result};

use crate::config::WgPeerConfig;
use crate::control::WireGuardControl;

/// WireGuard 接口的建立方式。建好接口后两者的 `wg`/`ip` 操作完全一致，
/// 仅"创建接口"这一步不同。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WgMode {
    /// 内核 WireGuard：`ip link add type wireguard`。需内核 WireGuard 模块
    /// （Linux ≥ 5.6 或 `wireguard-dkms`）+ `CAP_NET_ADMIN`，性能最佳。
    Kernel,
    /// 用户态 WireGuard：`wireguard-go <iface>`。只需 `/dev/net/tun`，
    /// 不依赖内核 WG 模块，适配 CentOS 7（3.10）等老内核；性能略低于内核态。
    Userspace,
}

/// 基于 `wg`/`ip` 命令的 WireGuard 控制平面（内核态或用户态接口均可）。
pub struct KernelWireGuardControl {
    iface: String,
    server_public_key: String,
}

impl KernelWireGuardControl {
    /// 创建并配置 WireGuard 接口（幂等：已存在则先删除重建）。
    ///
    /// - `iface`：接口名，如 `wg0`
    /// - `server_private_key` / `server_public_key`：base64
    /// - `server_addr` + `subnet_prefix`：接口地址，如 `10.8.0.1/24`
    /// - `listen_port`：UDP 监听端口
    /// - `mode`：内核态或用户态（[`WgMode`]）
    pub async fn start(
        iface: &str,
        server_private_key: &str,
        server_public_key: &str,
        server_addr: Ipv4Addr,
        subnet_prefix: u8,
        listen_port: u16,
        mode: WgMode,
    ) -> Result<Self> {
        // 幂等清理：忽略「不存在」错误
        let _ = run("ip", &["link", "del", iface]).await;

        create_interface(iface, mode).await?;

        // 私钥写临时文件（0600），避免进入命令行
        let key_path = write_private_key_tempfile(iface, server_private_key)?;
        let listen = listen_port.to_string();
        let set_res = run(
            "wg",
            &[
                "set",
                iface,
                "private-key",
                &key_path,
                "listen-port",
                &listen,
            ],
        )
        .await;
        // 无论成功与否都删除私钥文件
        let _ = std::fs::remove_file(&key_path);
        set_res?;

        let addr = format!("{server_addr}/{subnet_prefix}");
        run("ip", &["addr", "add", &addr, "dev", iface]).await?;
        run("ip", &["link", "set", iface, "up"]).await?;

        tracing::info!(iface, %addr, listen_port, ?mode, "WireGuard 接口已就绪");

        Ok(Self {
            iface: iface.to_string(),
            server_public_key: server_public_key.to_string(),
        })
    }
}

/// 按 [`WgMode`] 创建 WireGuard 接口。内核态用 `ip link add type wireguard`（需内核模块）；
/// 用户态用 `wireguard-go <iface>`（仅需 `/dev/net/tun`，建好接口后父进程退出、守护进程转后台）。
async fn create_interface(iface: &str, mode: WgMode) -> Result<()> {
    match mode {
        WgMode::Kernel => {
            run("ip", &["link", "add", "dev", iface, "type", "wireguard"]).await?;
        }
        WgMode::Userspace => {
            // wireguard-go 的 UAPI socket 在 /var/run/wireguard/<iface>.sock；
            // 进程被 kill -9 时不会自清，残留 socket 会让 wireguard-go 拒绝重建。
            let _ = std::fs::create_dir_all("/var/run/wireguard");
            let _ = std::fs::remove_file(format!("/var/run/wireguard/{iface}.sock"));
            spawn_wireguard_go(iface).await?;
        }
    }
    Ok(())
}

#[async_trait]
impl WireGuardControl for KernelWireGuardControl {
    async fn configure_peer(&self, cfg: &WgPeerConfig) -> Result<()> {
        // allowed-ips = <vpn_ip>/32 + 各 LAN 网段（逗号分隔，cryptokey routing）
        let allowed = cfg.allowed_ips().join(",");
        run(
            "wg",
            &[
                "set",
                &self.iface,
                "peer",
                &cfg.public_key,
                "allowed-ips",
                &allowed,
            ],
        )
        .await?;
        // 站点 LAN 网段需在 OS 路由表加 `<subnet> dev <iface>`，否则内核不会把发往该网段的包送进 wg 接口
        // （`wg set` 只配 cryptokey routing，不像 `wg-quick` 自动加路由）。peer 自己的 /32 在接口子网内无需额外路由。
        for subnet in &cfg.allowed_subnets {
            run("ip", &["route", "replace", subnet, "dev", &self.iface]).await?;
        }
        tracing::debug!(public_key = %cfg.public_key, vpn_ip = %cfg.vpn_ip, allowed = %allowed, "内核 configure_peer");
        Ok(())
    }

    async fn remove_peer(&self, public_key: &str) -> Result<()> {
        run("wg", &["set", &self.iface, "peer", public_key, "remove"]).await?;
        tracing::debug!(public_key, "内核 remove_peer");
        Ok(())
    }

    async fn list_peers(&self) -> Result<Vec<String>> {
        let out = run_output("wg", &["show", &self.iface, "peers"]).await?;
        Ok(out
            .lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect())
    }

    async fn remove_routes(&self, subnets: &[String]) -> Result<()> {
        for subnet in subnets {
            // best-effort：路由可能已被其他途径删除/从未添加，`ip route del` 失败不应致命。
            if let Err(e) = run("ip", &["route", "del", subnet, "dev", &self.iface]).await {
                tracing::warn!(subnet = %subnet, error = %e, "删除残留站点路由失败（忽略）");
            }
        }
        Ok(())
    }

    fn server_public_key(&self) -> &str {
        &self.server_public_key
    }
}

/// 把私钥写入权限 0600 的临时文件，返回路径。
///
/// unix 下用 `O_EXCL`(create_new)**原子创建并建时即 0600**:避免「先 0644 落盘、再 chmod」
/// 之间的 world-readable 窗口,以及对预置符号链接的跟随写入(TOCTOU)。路径可预测,故先
/// best-effort 删掉本进程上一轮的残留;若届时该路径已被他人(如攻击者符号链接)占用,
/// `O_EXCL` 会让 open 失败 → 直接报错而非写穿,fail-closed。
fn write_private_key_tempfile(iface: &str, private_key: &str) -> Result<String> {
    let path = std::env::temp_dir().join(format!("vpn-wg-{iface}.key"));
    let _ = std::fs::remove_file(&path);
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|e| AppError::WireGuard(format!("创建临时私钥失败: {e}")))?;
        f.write_all(private_key.as_bytes())
            .map_err(|e| AppError::WireGuard(format!("写入临时私钥失败: {e}")))?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&path, private_key)
            .map_err(|e| AppError::WireGuard(format!("写入临时私钥失败: {e}")))?;
    }
    Ok(path.to_string_lossy().into_owned())
}

/// 启动 wireguard-go 用户态守护进程。
///
/// **不**用 [`run`]:`run` 走 `Command::output()`,会一直等到子进程的 stdout/stderr 到 EOF;
/// wireguard-go 前台模式(`WG_PROCESS_FOREGROUND=1`,容器/systemd 常见)长开这两个 fd,
/// `output()` 永不返回 → 服务端启动无超时挂死。这里改为 spawn + stdio 置空 + 短超时等待:
/// 默认会 daemonize(父进程很快退出);前台模式则在超时后视为已就绪并继续(不杀该进程,
/// drop Child 不会终止它,因未设 kill_on_drop)。
async fn spawn_wireguard_go(iface: &str) -> Result<()> {
    use std::process::Stdio;
    let mut child = tokio::process::Command::new("wireguard-go")
        .arg(iface)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| AppError::WireGuard(format!("启动 wireguard-go 失败: {e}")))?;
    match tokio::time::timeout(std::time::Duration::from_secs(5), child.wait()).await {
        Ok(Ok(status)) if !status.success() => Err(AppError::WireGuard(format!(
            "wireguard-go 异常退出 (status {status})"
        ))),
        Ok(Ok(_)) => Ok(()), // 正常 daemonize:父进程 0 退出
        Ok(Err(e)) => Err(AppError::WireGuard(format!("等待 wireguard-go 失败: {e}"))),
        Err(_) => {
            tracing::info!(iface, "wireguard-go 前台模式运行（等待超时，视为已就绪）");
            Ok(())
        }
    }
}

/// 执行命令，非零退出码映射为 [`AppError::WireGuard`]。
async fn run(cmd: &str, args: &[&str]) -> Result<()> {
    let output = tokio::process::Command::new(cmd)
        .args(args)
        .output()
        .await
        .map_err(|e| AppError::WireGuard(format!("执行 {cmd} 失败: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AppError::WireGuard(format!(
            "{cmd} {args:?} 失败 (status {}): {}",
            output.status,
            stderr.trim()
        )));
    }
    Ok(())
}

/// 执行命令并返回 stdout。
async fn run_output(cmd: &str, args: &[&str]) -> Result<String> {
    let output = tokio::process::Command::new(cmd)
        .args(args)
        .output()
        .await
        .map_err(|e| AppError::WireGuard(format!("执行 {cmd} 失败: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AppError::WireGuard(format!(
            "{cmd} {args:?} 失败 (status {}): {}",
            output.status,
            stderr.trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
