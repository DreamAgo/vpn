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

        Ok(Self {
            bind_addr,
            database_url,
            enable_https,
            domain,
            data_dir,
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
        }
        let cfg = ServerConfig::from_env().unwrap();
        assert_eq!(cfg.bind_addr, "0.0.0.0:8080");
        assert!(!cfg.enable_https);
    }
}
