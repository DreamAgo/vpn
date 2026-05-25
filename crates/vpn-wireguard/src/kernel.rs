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

/// 基于内核 WireGuard 的控制平面。
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
    pub async fn start(
        iface: &str,
        server_private_key: &str,
        server_public_key: &str,
        server_addr: Ipv4Addr,
        subnet_prefix: u8,
        listen_port: u16,
    ) -> Result<Self> {
        // 幂等清理：忽略「不存在」错误
        let _ = run("ip", &["link", "del", iface]).await;

        run("ip", &["link", "add", "dev", iface, "type", "wireguard"]).await?;

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

        tracing::info!(iface, %addr, listen_port, "内核 WireGuard 接口已就绪");

        Ok(Self {
            iface: iface.to_string(),
            server_public_key: server_public_key.to_string(),
        })
    }
}

#[async_trait]
impl WireGuardControl for KernelWireGuardControl {
    async fn configure_peer(&self, cfg: &WgPeerConfig) -> Result<()> {
        let allowed = format!("{}/32", cfg.vpn_ip);
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
        tracing::debug!(public_key = %cfg.public_key, vpn_ip = %cfg.vpn_ip, "内核 configure_peer");
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

    fn server_public_key(&self) -> &str {
        &self.server_public_key
    }
}

/// 把私钥写入权限 0600 的临时文件，返回路径。
fn write_private_key_tempfile(iface: &str, private_key: &str) -> Result<String> {
    let path = std::env::temp_dir().join(format!("vpn-wg-{iface}.key"));
    std::fs::write(&path, private_key)
        .map_err(|e| AppError::WireGuard(format!("写入临时私钥失败: {e}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| AppError::WireGuard(format!("设置私钥文件权限失败: {e}")))?;
    }
    Ok(path.to_string_lossy().into_owned())
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
