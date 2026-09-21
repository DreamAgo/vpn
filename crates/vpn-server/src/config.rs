//! 服务端配置：环境变量 + 默认值。

use base64::Engine;
use std::env;
use vpn_api_types::peer::ObfsMode;
use vpn_api_types::system::ObfsNetworkSettings;
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

/// 服务端 bootstrap 与一次性种子配置。
/// 数据库连接前置项继续来自环境变量；非敏感数据面字段只保留在 seed 中。
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
    /// 审计日志保留天数（Story 5.3 清理任务）。默认 180。
    pub audit_retention_days: u32,
    /// 隧道 MTU 环境变量原始值。仅在数据库尚无整组配置时解析并作为一次性种子。
    pub network_settings_seed: NetworkSettingsSeed,
    /// 非敏感数据面配置的一次性原始种子；只有 v2 数据不存在时才解析。
    pub data_plane_seed: DataPlaneSettingsSeed,
    /// 事件通知配置（SMTP 邮件）。
    pub notifications: NotificationConfig,
    /// 飞书 OAuth。三项同时存在时启用。
    pub feishu: FeishuConfig,
    /// 飞书审批外部选项 webhook。
    pub feishu_approval_options: FeishuApprovalOptionsConfig,
    /// 飞书审批事件与实例字段配置。
    pub feishu_approval: FeishuApprovalConfig,
}

#[derive(Debug, Clone, Default)]
pub struct NetworkSettingsSeed {
    pub mode: Option<String>,
    pub default_mtu: Option<String>,
    pub min_mtu: Option<String>,
    pub max_mtu: Option<String>,
}

#[derive(Clone, Default)]
pub struct DataPlaneSettingsSeed {
    pub vpn_subnet: Option<String>,
    pub vpn_listen_port: Option<String>,
    pub vpn_endpoint: Option<String>,
    pub wg_backend: Option<String>,
    pub wg_interface: Option<String>,
    pub obfs_enabled: Option<String>,
    pub obfs_mode: Option<String>,
    pub obfs_bind_addr: Option<String>,
    pub obfs_endpoint: Option<String>,
    pub obfs_path_mtu: Option<String>,
    pub server_routes: Option<String>,
    pub dns_mode: Option<String>,
    pub dns_default_upstreams: Option<String>,
    pub dns_forward_rules: Option<String>,
    pub dns_static_records: Option<String>,
    pub obfs_psk: Option<Zeroizing<String>>,
}

impl std::fmt::Debug for DataPlaneSettingsSeed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DataPlaneSettingsSeed")
            .field("vpn_subnet", &self.vpn_subnet)
            .field("vpn_listen_port", &self.vpn_listen_port)
            .field("vpn_endpoint", &self.vpn_endpoint)
            .field("wg_backend", &self.wg_backend)
            .field("wg_interface", &self.wg_interface)
            .field("obfs_enabled", &self.obfs_enabled)
            .field("obfs_mode", &self.obfs_mode)
            .field("obfs_bind_addr", &self.obfs_bind_addr)
            .field("obfs_endpoint", &self.obfs_endpoint)
            .field("obfs_path_mtu", &self.obfs_path_mtu)
            .field("server_routes", &self.server_routes)
            .field("dns_mode", &self.dns_mode)
            .field("dns_default_upstreams", &self.dns_default_upstreams)
            .field(
                "dns_forward_rules_configured",
                &self.dns_forward_rules.is_some(),
            )
            .field(
                "dns_static_records_configured",
                &self.dns_static_records.is_some(),
            )
            .field("obfs_psk_configured", &self.obfs_psk.is_some())
            .finish()
    }
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
    pub max_devices_control_id: Option<String>,
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
    /// 将数据库中的非秘密混淆配置与秘密环境变量组合为运行时配置。
    pub fn obfs_config(
        &self,
        settings: &ObfsNetworkSettings,
    ) -> anyhow::Result<Option<ObfsConfig>> {
        if !settings.enabled {
            return Ok(None);
        }
        let encoded = self
            .data_plane_seed
            .obfs_psk
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("启用 UDP 混淆时必须设置 VPN_OBFS_PSK"))?;
        let decoded = Zeroizing::new(
            base64::engine::general_purpose::STANDARD
                .decode(encoded.trim())
                .map_err(|_| anyhow::anyhow!("VPN_OBFS_PSK 必须是 32 字节标准 Base64"))?,
        );
        let psk: [u8; 32] = decoded
            .as_slice()
            .try_into()
            .map_err(|_| anyhow::anyhow!("VPN_OBFS_PSK 解码后必须恰好为 32 字节"))?;
        Ok(Some(ObfsConfig {
            bind_addr: settings.bind_addr.clone(),
            public_endpoint: settings.public_endpoint.clone(),
            mode: settings.mode,
            path_mtu: settings.path_mtu,
            psk: Zeroizing::new(psk),
            max_sessions: 4096,
            new_sessions_per_ip_per_minute: 20,
            session_idle_secs: 180,
        }))
    }

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
            .filter(|days| *days > 0)
            .unwrap_or(180);

        let network_settings_seed = NetworkSettingsSeed {
            mode: env::var("VPN_TUN_MTU_MODE").ok(),
            default_mtu: env::var("VPN_TUN_MTU_DEFAULT").ok(),
            min_mtu: env::var("VPN_TUN_MTU_MIN").ok(),
            max_mtu: env::var("VPN_TUN_MTU_MAX").ok(),
        };
        let data_plane_seed = DataPlaneSettingsSeed {
            vpn_subnet: Some(env::var("VPN_SUBNET").unwrap_or_else(|_| "10.8.0.0/24".into())),
            vpn_listen_port: Some(env::var("VPN_LISTEN_PORT").unwrap_or_else(|_| "51820".into())),
            vpn_endpoint: Some(vpn_endpoint.clone()),
            wg_backend: Some(env::var("VPN_WG_BACKEND").unwrap_or_else(|_| "noop".into())),
            wg_interface: Some(env::var("VPN_WG_INTERFACE").unwrap_or_else(|_| "wg0".into())),
            obfs_enabled: Some(env::var("VPN_OBFS_ENABLED").unwrap_or_else(|_| "false".into())),
            obfs_mode: Some(env::var("VPN_OBFS_MODE").unwrap_or_else(|_| "low-overhead-v1".into())),
            obfs_bind_addr: Some(
                env::var("VPN_OBFS_BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:47358".into()),
            ),
            obfs_endpoint: Some(
                env::var("VPN_OBFS_ENDPOINT").unwrap_or_else(|_| {
                    format!("{}:47358", domain.as_deref().unwrap_or("127.0.0.1"))
                }),
            ),
            obfs_path_mtu: Some(env::var("VPN_OBFS_PATH_MTU").unwrap_or_else(|_| "1500".into())),
            server_routes: Some(env::var("VPN_SERVER_ROUTES").unwrap_or_default()),
            dns_mode: env::var("VPN_DNS_MODE").ok(),
            dns_default_upstreams: env::var("VPN_DNS_DEFAULT_UPSTREAMS").ok(),
            dns_forward_rules: env::var("VPN_DNS_FORWARD_RULES").ok(),
            dns_static_records: env::var("VPN_DNS_STATIC_RECORDS").ok(),
            obfs_psk: env::var("VPN_OBFS_PSK").ok().map(Zeroizing::new),
        };
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
            max_devices_control_id: optional_non_blank(
                env::var("VPN_FEISHU_APPROVAL_MAX_DEVICES_CONTROL_ID").ok(),
            ),
            verification_token: optional_non_blank(
                env::var("VPN_FEISHU_APPROVAL_VERIFICATION_TOKEN").ok(),
            ),
            encrypt_key: optional_non_blank(env::var("VPN_FEISHU_APPROVAL_ENCRYPT_KEY").ok()),
        };
        Ok(Self {
            bind_addr,
            database_url,
            enable_https,
            domain,
            data_dir,
            audit_retention_days,
            network_settings_seed,
            data_plane_seed,
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
        assert_eq!(
            cfg.data_plane_seed.vpn_subnet.as_deref(),
            Some("10.8.0.0/24")
        );
        assert_eq!(
            cfg.data_plane_seed.vpn_listen_port.as_deref(),
            Some("51820")
        );
        assert_eq!(
            cfg.data_plane_seed.vpn_endpoint.as_deref(),
            Some("127.0.0.1:51820")
        );
        assert_eq!(cfg.audit_retention_days, 180);
        assert_eq!(cfg.data_plane_seed.wg_backend.as_deref(), Some("noop"));
        assert_eq!(cfg.data_plane_seed.wg_interface.as_deref(), Some("wg0"));
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
}
