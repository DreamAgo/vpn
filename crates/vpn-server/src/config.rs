//! 服务端配置：环境变量 + 默认值。

use std::env;

/// 服务端启动配置。
///
/// 来源优先级：
/// 1. 环境变量（最高）
/// 2. 编译期默认值
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// 监听地址。默认 `0.0.0.0:8080`（开发用，无 HTTPS）。
    pub bind_addr: String,
    /// SQLite 数据库 URL。默认 `sqlite://./dev.db`（自动创建）。
    pub database_url: String,
    /// 是否启用 HTTPS（生产环境 true，开发环境 false）。
    pub enable_https: bool,
    /// 公网域名（启用 HTTPS 时必需，用于 ACME 申请）。
    pub domain: Option<String>,
    /// 数据目录（用于密钥、ACME 证书缓存）。
    pub data_dir: String,
    /// VPN 虚拟子网（CIDR）。默认 `10.8.0.0/24`。
    pub vpn_subnet: String,
    /// WireGuard 监听 UDP 端口。默认 `51820`。
    pub vpn_listen_port: u16,
    /// 服务端 WireGuard endpoint（host:port），客户端据此连接。
    ///
    /// 若未显式设置 `VPN_ENDPOINT`，则用 `VPN_DOMAIN:vpn_listen_port`（若有域名），
    /// 否则回退占位 `127.0.0.1:vpn_listen_port`（开发用）。
    pub vpn_endpoint: String,
    /// 审计日志保留天数（Story 5.3 清理任务）。默认 180。
    pub audit_retention_days: u32,
}

impl ServerConfig {
    /// 从环境变量加载配置。
    ///
    /// # Errors
    /// 如果启用 HTTPS 但缺少 `VPN_DOMAIN`，返回错误。
    pub fn from_env() -> anyhow::Result<Self> {
        let bind_addr = env::var("VPN_BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string());
        let database_url =
            env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite://./dev.db?mode=rwc".to_string());
        let enable_https = env::var("VPN_HTTPS")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);
        let domain = env::var("VPN_DOMAIN").ok();
        let data_dir = env::var("VPN_DATA_DIR").unwrap_or_else(|_| "./data".to_string());

        if enable_https && domain.is_none() {
            anyhow::bail!("启用 HTTPS (VPN_HTTPS=true) 需要 VPN_DOMAIN 环境变量");
        }

        let vpn_subnet = env::var("VPN_SUBNET").unwrap_or_else(|_| "10.8.0.0/24".to_string());
        let vpn_listen_port = env::var("VPN_LISTEN_PORT")
            .ok()
            .and_then(|v| v.parse::<u16>().ok())
            .unwrap_or(51820);
        let vpn_endpoint = env::var("VPN_ENDPOINT").ok().unwrap_or_else(|| {
            let host = domain.clone().unwrap_or_else(|| "127.0.0.1".to_string());
            format!("{host}:{vpn_listen_port}")
        });

        let audit_retention_days = env::var("VPN_AUDIT_RETENTION_DAYS")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(180);

        Ok(Self {
            bind_addr,
            database_url,
            enable_https,
            domain,
            data_dir,
            vpn_subnet,
            vpn_listen_port,
            vpn_endpoint,
            audit_retention_days,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_no_env_vars() {
        // 清理可能的环境变量影响
        unsafe {
            env::remove_var("VPN_BIND_ADDR");
            env::remove_var("VPN_HTTPS");
            env::remove_var("DATABASE_URL");
            env::remove_var("VPN_DOMAIN");
            env::remove_var("VPN_DATA_DIR");
            env::remove_var("VPN_SUBNET");
            env::remove_var("VPN_LISTEN_PORT");
            env::remove_var("VPN_ENDPOINT");
            env::remove_var("VPN_AUDIT_RETENTION_DAYS");
        }
        let cfg = ServerConfig::from_env().unwrap();
        assert_eq!(cfg.bind_addr, "0.0.0.0:8080");
        assert!(!cfg.enable_https);
        assert_eq!(cfg.vpn_subnet, "10.8.0.0/24");
        assert_eq!(cfg.vpn_listen_port, 51820);
        assert_eq!(cfg.vpn_endpoint, "127.0.0.1:51820");
        assert_eq!(cfg.audit_retention_days, 180);
    }
}
