//! ACME 自动 HTTPS 证书管理（基于 rustls-acme + Let's Encrypt）。
//!
//! 设计：
//! - 服务端启动时检测 VPN_HTTPS=true + VPN_DOMAIN
//! - rustls-acme 自动申请并管理证书（首次申请 + 自动续期）
//! - 证书缓存到 {data_dir}/acme/
//!
//! 注：本模块依赖外部网络（Let's Encrypt API）+ 真实域名 DNS 解析，
//! 集成测试需真实部署环境，本 Story 仅做编译验证。

use std::path::PathBuf;

use rustls_acme::{caches::DirCache, AcmeConfig};

/// 构造 ACME 配置，由调用方驱动证书申请与 incoming TLS 流。
///
/// # 参数
/// - `domain`: 公网域名（必须 DNS 已解析到本机）
/// - `email`: 联系邮箱（Let's Encrypt 用于通知）
/// - `cache_dir`: 证书缓存目录（持久化，重启不重申请）
/// - `production`: true 用 Let's Encrypt 生产环境；false 用 staging（测试用）
pub fn acme_config(
    domain: &str,
    email: &str,
    cache_dir: PathBuf,
    production: bool,
) -> AcmeConfig<std::io::Error> {
    let directory_lets_encrypt = if production {
        "https://acme-v02.api.letsencrypt.org/directory"
    } else {
        "https://acme-staging-v02.api.letsencrypt.org/directory"
    };

    AcmeConfig::new([domain.to_string()])
        .contact_push(format!("mailto:{}", email))
        .cache(DirCache::new(cache_dir))
        .directory(directory_lets_encrypt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn acme_config_builder_compiles() {
        let temp_dir = TempDir::new().unwrap();
        let _cfg = acme_config(
            "vpn.example.com",
            "admin@example.com",
            temp_dir.path().to_path_buf(),
            false, // staging
        );
        // 仅验证 builder API 正确（不真实发起 ACME 请求）
    }
}
