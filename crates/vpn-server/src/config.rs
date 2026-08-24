//! 服务端配置：环境变量 + 默认值。

use std::env;
use std::net::SocketAddr;

use base64::Engine;
use vpn_api_types::peer::ObfsMode;
use zeroize::Zeroizing;

/// UDP 混淆监听配置。PSK 的 Debug 输出始终脱敏。
#[derive(Clone)]
pub struct ObfsConfig {
    pub bind_addr: String,
    pub public_endpoint: String,
    pub mode: ObfsMode,
    pub path_mtu: u16,
    pub psk: Zeroizing<[u8; 32]>,
    pub max_sessions: usize,
    pub new_sessions_per_ip_per_minute: u32,
    pub session_idle_secs: u64,
}

impl std::fmt::Debug for ObfsConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ObfsConfig")
            .field("bind_addr", &self.bind_addr)
            .field("public_endpoint", &self.public_endpoint)
            .field("mode", &self.mode)
            .field("path_mtu", &self.path_mtu)
            .field("max_sessions", &self.max_sessions)
            .field(
                "new_sessions_per_ip_per_minute",
                &self.new_sessions_per_ip_per_minute,
            )
            .field("session_idle_secs", &self.session_idle_secs)
            .finish_non_exhaustive()
    }
}

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
    /// 可选的 Rust 原生 UDP 混淆传输。
    pub obfs: Option<ObfsConfig>,
    /// 审计日志保留天数（Story 5.3 清理任务）。默认 180。
    pub audit_retention_days: u32,
    /// WireGuard 后端："noop"（默认，仅记账，无需特权）或 "kernel"（Linux 内核 WireGuard，需 root/CAP_NET_ADMIN + wg 工具）。
    pub wg_backend: String,
    /// WireGuard 接口名。默认 `wg0`。
    pub wg_interface: String,
    /// 服务端自身网关的网段（CIDR 列表，如所在 Docker 网络），
    /// 会作为 allowed_routes 下发给客户端，使其经隧道访问这些网段。默认空。
    pub server_routes: Vec<String>,
    /// 事件通知配置（SMTP 邮件）。
    pub notifications: NotificationConfig,
    /// 飞书 OAuth。三项同时存在时启用。
    pub feishu: FeishuConfig,
    /// 飞书审批外部选项 webhook。
    pub feishu_approval_options: FeishuApprovalOptionsConfig,
    /// 飞书审批事件与实例字段配置。
    pub feishu_approval: FeishuApprovalConfig,
}

#[derive(Clone, Default)]
pub struct FeishuConfig {
    pub app_id: Option<String>,
    pub app_secret: Option<String>,
    pub redirect_uri: Option<String>,
}

impl std::fmt::Debug for FeishuConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FeishuConfig")
            .field("enabled", &self.enabled())
            .field("app_id", &self.app_id)
            .finish_non_exhaustive()
    }
}

impl FeishuConfig {
    pub fn enabled(&self) -> bool {
        self.app_id.as_deref().is_some_and(|v| !v.trim().is_empty())
            && self
                .app_secret
                .as_deref()
                .is_some_and(|v| !v.trim().is_empty())
            && self
                .redirect_uri
                .as_deref()
                .is_some_and(|v| !v.trim().is_empty())
    }
}

#[derive(Clone, Default)]
pub struct FeishuApprovalOptionsConfig {
    pub token: Option<String>,
}

#[derive(Clone, Default)]
pub struct FeishuApprovalConfig {
    pub approval_code: Option<String>,
    pub group_control_id: Option<String>,
    pub expiry_control_id: Option<String>,
    pub reason_control_id: Option<String>,
    pub verification_token: Option<String>,
    pub encrypt_key: Option<String>,
}

impl FeishuApprovalConfig {
    pub fn enabled(&self) -> bool {
        [
            self.approval_code.as_deref(),
            self.group_control_id.as_deref(),
            self.expiry_control_id.as_deref(),
            self.reason_control_id.as_deref(),
            self.verification_token.as_deref(),
            self.encrypt_key.as_deref(),
        ]
        .into_iter()
        .all(|value| value.is_some_and(|value| !value.trim().is_empty()))
    }
}

impl std::fmt::Debug for FeishuApprovalConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FeishuApprovalConfig")
            .field("enabled", &self.enabled())
            .field("approval_code", &self.approval_code)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for FeishuApprovalOptionsConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FeishuApprovalOptionsConfig")
            .field("configured", &self.token.is_some())
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct NotificationConfig {
    pub email_enabled: bool,
    pub smtp_host: Option<String>,
    pub smtp_port: u16,
    pub smtp_username: Option<String>,
    pub smtp_password: Option<String>,
    pub email_from: Option<String>,
    pub email_to: Vec<String>,
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
        let obfs = if env_bool("VPN_OBFS_ENABLED", false) {
            if !enable_https {
                anyhow::bail!("启用 UDP 混淆时必须启用 HTTPS，禁止通过 HTTP 下发 PSK");
            }
            let encoded = Zeroizing::new(
                env::var("VPN_OBFS_PSK")
                    .map_err(|_| anyhow::anyhow!("启用 UDP 混淆时必须设置 VPN_OBFS_PSK"))?,
            );
            let decoded = Zeroizing::new(
                base64::engine::general_purpose::STANDARD
                    .decode(encoded.trim())
                    .map_err(|_| anyhow::anyhow!("VPN_OBFS_PSK 必须是 32 字节标准 Base64"))?,
            );
            let psk: [u8; 32] = decoded
                .as_slice()
                .try_into()
                .map_err(|_| anyhow::anyhow!("VPN_OBFS_PSK 解码后必须恰好为 32 字节"))?;
            let mode = match env::var("VPN_OBFS_MODE")
                .unwrap_or_else(|_| "low-overhead-v1".to_string())
                .as_str()
            {
                "low-overhead-v1" => ObfsMode::LowOverheadV1,
                "paranoid-v1" => ObfsMode::ParanoidV1,
                other => anyhow::bail!("VPN_OBFS_MODE 不支持: {other}"),
            };
            let bind_addr =
                env::var("VPN_OBFS_BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:47358".to_string());
            let parsed_bind: SocketAddr = bind_addr
                .parse()
                .map_err(|_| anyhow::anyhow!("VPN_OBFS_BIND_ADDR 必须是 IPv4 socket 地址"))?;
            if !parsed_bind.is_ipv4() {
                anyhow::bail!("VPN_OBFS_BIND_ADDR 当前仅支持 IPv4");
            }
            let public_endpoint = env::var("VPN_OBFS_ENDPOINT").unwrap_or_else(|_| {
                let host = domain.as_deref().unwrap_or("127.0.0.1");
                format!("{host}:47358")
            });
            validate_ipv4_endpoint(&public_endpoint)?;
            let path_mtu = match env::var("VPN_OBFS_PATH_MTU") {
                Ok(value) => value
                    .parse()
                    .map_err(|_| anyhow::anyhow!("VPN_OBFS_PATH_MTU 必须是整数"))?,
                Err(env::VarError::NotPresent) => 1500,
                Err(error) => return Err(error.into()),
            };
            if !(576..=9000).contains(&path_mtu) {
                anyhow::bail!("VPN_OBFS_PATH_MTU 必须在 576..=9000");
            }
            Some(ObfsConfig {
                bind_addr,
                public_endpoint,
                mode,
                path_mtu,
                psk: Zeroizing::new(psk),
                max_sessions: 4096,
                new_sessions_per_ip_per_minute: 20,
                session_idle_secs: 180,
            })
        } else {
            None
        };

        let audit_retention_days = env::var("VPN_AUDIT_RETENTION_DAYS")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(180);

        let wg_backend = env::var("VPN_WG_BACKEND").unwrap_or_else(|_| "noop".to_string());
        if obfs.is_some() && wg_backend == "noop" {
            anyhow::bail!("启用 UDP 混淆时必须配置真实 WireGuard 后端，不能使用 noop");
        }
        let wg_interface = env::var("VPN_WG_INTERFACE").unwrap_or_else(|_| "wg0".to_string());
        let server_routes = env::var("VPN_SERVER_ROUTES")
            .ok()
            .map(|v| {
                v.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let notifications = NotificationConfig {
            email_enabled: env_bool("VPN_NOTIFY_EMAIL_ENABLED", false),
            smtp_host: env::var("VPN_SMTP_HOST").ok(),
            smtp_port: env::var("VPN_SMTP_PORT")
                .ok()
                .and_then(|v| v.parse::<u16>().ok())
                .unwrap_or(587),
            smtp_username: env::var("VPN_SMTP_USERNAME").ok(),
            smtp_password: env::var("VPN_SMTP_PASSWORD").ok(),
            email_from: env::var("VPN_NOTIFY_EMAIL_FROM").ok(),
            email_to: env::var("VPN_NOTIFY_EMAIL_TO")
                .ok()
                .map(|v| {
                    v.split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
        };
        let feishu = FeishuConfig {
            app_id: env::var("VPN_FEISHU_APP_ID").ok(),
            app_secret: env::var("VPN_FEISHU_APP_SECRET").ok(),
            redirect_uri: env::var("VPN_FEISHU_REDIRECT_URI").ok(),
        };
        let feishu_approval_options = FeishuApprovalOptionsConfig {
            token: optional_non_blank(env::var("VPN_FEISHU_APPROVAL_OPTIONS_TOKEN").ok()),
        };
        validate_approval_options_token(feishu_approval_options.token.as_deref())?;
        let feishu_approval = FeishuApprovalConfig {
            approval_code: optional_non_blank(env::var("VPN_FEISHU_APPROVAL_CODE").ok()),
            group_control_id: optional_non_blank(
                env::var("VPN_FEISHU_APPROVAL_GROUP_CONTROL_ID").ok(),
            ),
            expiry_control_id: optional_non_blank(
                env::var("VPN_FEISHU_APPROVAL_EXPIRY_CONTROL_ID").ok(),
            ),
            reason_control_id: optional_non_blank(
                env::var("VPN_FEISHU_APPROVAL_REASON_CONTROL_ID").ok(),
            ),
            verification_token: optional_non_blank(
                env::var("VPN_FEISHU_APPROVAL_VERIFICATION_TOKEN").ok(),
            ),
            encrypt_key: optional_non_blank(env::var("VPN_FEISHU_APPROVAL_ENCRYPT_KEY").ok()),
        };
        let approval_fields = [
            feishu_approval.approval_code.is_some(),
            feishu_approval.group_control_id.is_some(),
            feishu_approval.expiry_control_id.is_some(),
            feishu_approval.reason_control_id.is_some(),
            feishu_approval.verification_token.is_some(),
            feishu_approval.encrypt_key.is_some(),
        ];
        if approval_fields.iter().any(|set| *set) && !feishu_approval.enabled() {
            anyhow::bail!("飞书审批配置必须六项同时设置");
        }
        if feishu_approval.enabled() && wg_backend != "kernel" {
            anyhow::bail!("飞书审批网络授权首期仅支持 VPN_WG_BACKEND=kernel");
        }
        if feishu_approval.enabled() && !feishu.enabled() {
            anyhow::bail!(
                "飞书审批需要同时配置 VPN_FEISHU_APP_ID、VPN_FEISHU_APP_SECRET 与 VPN_FEISHU_REDIRECT_URI"
            );
        }
        if feishu_approval
            .verification_token
            .as_deref()
            .is_some_and(|value| value.len() < 16)
            || feishu_approval
                .encrypt_key
                .as_deref()
                .is_some_and(|value| value.len() < 16)
        {
            anyhow::bail!("飞书审批 Verification Token 与 Encrypt Key 至少需要 16 个字符");
        }

        if feishu.enabled() {
            if let Some(uri) = feishu.redirect_uri.as_deref() {
                if !uri.trim().starts_with("https://") {
                    anyhow::bail!("VPN_FEISHU_REDIRECT_URI 必须使用 HTTPS");
                }
            }
        }

        Ok(Self {
            bind_addr,
            database_url,
            enable_https,
            domain,
            data_dir,
            vpn_subnet,
            vpn_listen_port,
            vpn_endpoint,
            obfs,
            audit_retention_days,
            wg_backend,
            wg_interface,
            server_routes,
            notifications,
            feishu,
            feishu_approval_options,
            feishu_approval,
        })
    }
}

fn optional_non_blank(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn validate_approval_options_token(token: Option<&str>) -> anyhow::Result<()> {
    if token.is_some_and(|token| token.len() < 32) {
        anyhow::bail!("VPN_FEISHU_APPROVAL_OPTIONS_TOKEN 至少需要 32 个字符");
    }
    Ok(())
}

fn validate_ipv4_endpoint(endpoint: &str) -> anyhow::Result<()> {
    let (host, port) = endpoint
        .rsplit_once(':')
        .ok_or_else(|| anyhow::anyhow!("VPN_OBFS_ENDPOINT 必须是 host:port"))?;
    if host.is_empty()
        || host.contains(':')
        || port.parse::<u16>().ok().filter(|p| *p > 0).is_none()
    {
        anyhow::bail!("VPN_OBFS_ENDPOINT 必须是有效的 IPv4/域名 host:port");
    }
    Ok(())
}

fn env_bool(key: &str, default: bool) -> bool {
    env::var(key)
        .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(default)
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
            env::remove_var("VPN_OBFS_ENABLED");
            env::remove_var("VPN_OBFS_PSK");
            env::remove_var("VPN_OBFS_MODE");
            env::remove_var("VPN_OBFS_BIND_ADDR");
            env::remove_var("VPN_OBFS_ENDPOINT");
            env::remove_var("VPN_OBFS_PATH_MTU");
            env::remove_var("VPN_AUDIT_RETENTION_DAYS");
            env::remove_var("VPN_WG_BACKEND");
            env::remove_var("VPN_WG_INTERFACE");
            env::remove_var("VPN_SERVER_ROUTES");
            env::remove_var("VPN_FEISHU_APPROVAL_OPTIONS_TOKEN");
        }
        let cfg = ServerConfig::from_env().unwrap();
        assert_eq!(cfg.bind_addr, "0.0.0.0:8080");
        assert!(!cfg.enable_https);
        assert_eq!(cfg.vpn_subnet, "10.8.0.0/24");
        assert_eq!(cfg.vpn_listen_port, 51820);
        assert_eq!(cfg.vpn_endpoint, "127.0.0.1:51820");
        assert_eq!(cfg.audit_retention_days, 180);
        assert_eq!(cfg.wg_backend, "noop");
        assert_eq!(cfg.wg_interface, "wg0");
        assert!(cfg.feishu_approval_options.token.is_none());
    }

    #[test]
    fn feishu_config_requires_three_non_blank_values() {
        let complete = FeishuConfig {
            app_id: Some("app".into()),
            app_secret: Some("secret".into()),
            redirect_uri: Some("https://vpn.example.com/callback".into()),
        };
        assert!(complete.enabled());
        let debug = format!("{complete:?}");
        assert!(!debug.contains("secret"));
        assert!(!debug.contains("callback"));
        assert!(!FeishuConfig {
            app_id: Some("   ".into()),
            ..complete.clone()
        }
        .enabled());
        assert!(!FeishuConfig {
            app_secret: None,
            ..complete
        }
        .enabled());
    }

    #[test]
    fn optional_secret_trims_and_rejects_blank_values() {
        assert_eq!(
            optional_non_blank(Some("  secret  ".into())).as_deref(),
            Some("secret")
        );
        assert_eq!(optional_non_blank(Some("   ".into())), None);
        assert_eq!(optional_non_blank(None), None);
    }

    #[test]
    fn approval_options_debug_redacts_token() {
        let config = FeishuApprovalOptionsConfig {
            token: Some("super-secret".into()),
        };
        let debug = format!("{config:?}");
        assert!(debug.contains("configured: true"));
        assert!(!debug.contains("super-secret"));
    }

    #[test]
    fn approval_options_token_requires_minimum_length() {
        assert!(validate_approval_options_token(None).is_ok());
        assert!(validate_approval_options_token(Some(&"x".repeat(32))).is_ok());
        assert!(validate_approval_options_token(Some("too-short")).is_err());
    }
}
