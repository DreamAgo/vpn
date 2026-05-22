//! 启动校验：检查必要资源就绪，否则明确 fail。
//!
//! 当前实现仅校验绑定地址；后续 Story 添加：
//! - 数据库连接（Story 2.3）
//! - WireGuard 接口创建（Story 4.3）
//! - TLS 证书加载（Story 1.6）

use crate::config::ServerConfig;

/// 启动前校验。失败返回明确错误（不静默）。
pub fn validate(config: &ServerConfig) -> anyhow::Result<()> {
    // 校验 bind_addr 格式
    let _: std::net::SocketAddr = config
        .bind_addr
        .parse()
        .map_err(|e| anyhow::anyhow!("无效的 VPN_BIND_ADDR='{}': {}", config.bind_addr, e))?;

    tracing::info!(bind_addr = %config.bind_addr, "服务端配置校验通过");
    Ok(())
}
