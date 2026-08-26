//! 系统信息 DTO。

use serde::{Deserialize, Serialize};

use crate::peer::ObfsMode;
use std::net::Ipv4Addr;

pub const DEFAULT_TUN_MTU: u16 = 1360;
pub const MIN_TUN_MTU: u16 = 1280;
pub const MAX_TUN_MTU: u16 = 1420;
const IP_UDP_OVERHEAD: u16 = 28;
const OBFS_FRAME_OVERHEAD: u16 = 42;
const WG_OVERHEAD: u16 = 32;

/// 按当前混淆线协议计算不会超过外层 path MTU 的最大内层 MTU。
pub fn obfs_transport_safe_mtu(mode: ObfsMode, path_mtu: u16) -> u16 {
    let overhead = match mode {
        ObfsMode::LowOverheadV1 => IP_UDP_OVERHEAD + WG_OVERHEAD,
        ObfsMode::ParanoidV1 => IP_UDP_OVERHEAD + OBFS_FRAME_OVERHEAD + WG_OVERHEAD,
    };
    path_mtu.saturating_sub(overhead) & !15
}

/// 服务端下发的隧道 MTU 策略模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkMtuMode {
    Fixed,
    Auto,
}

/// 管理后台维护并下发给客户端的网络参数。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkSettings {
    pub mode: NetworkMtuMode,
    pub default_mtu: u16,
    pub min_mtu: u16,
    pub max_mtu: u16,
}

/// 需要重启服务端才能生效的 WireGuard 基础配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VpnBaseSettings {
    pub vpn_subnet: String,
    pub vpn_listen_port: u16,
    pub vpn_endpoint: String,
    pub wg_backend: String,
    pub wg_interface: String,
}

/// 可公开给管理员的混淆配置（不包含 PSK）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObfsNetworkSettings {
    pub enabled: bool,
    pub mode: ObfsMode,
    pub bind_addr: String,
    pub public_endpoint: String,
    pub path_mtu: u16,
}

/// 客户端应如何把 DNS 查询交给 VPN 内置解析器。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ClientDnsMode {
    #[default]
    Disabled,
    Global,
    Split,
}

/// 按域名后缀选择上游 DNS；最长后缀优先。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DnsForwardRule {
    pub domain: String,
    pub upstreams: Vec<String>,
}

/// 内置 DNS 返回的静态 IPv4 记录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DnsStaticRecord {
    pub name: String,
    pub address: Ipv4Addr,
    pub ttl: u32,
}

/// 服务端内置 DNS 与客户端 DNS 下发配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DnsNetworkSettings {
    pub mode: ClientDnsMode,
    #[serde(default)]
    pub split_domains: Vec<String>,
    #[serde(default)]
    pub default_upstreams: Vec<String>,
    #[serde(default)]
    pub forward_rules: Vec<DnsForwardRule>,
    #[serde(default)]
    pub static_records: Vec<DnsStaticRecord>,
}

/// 数据库持久化的数据面配置；秘密始终不进入此结构。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataPlaneSettings {
    pub vpn: VpnBaseSettings,
    pub obfs: ObfsNetworkSettings,
    pub mtu: NetworkSettings,
    #[serde(default)]
    pub dns: DnsNetworkSettings,
}

/// 管理页读取模型。LAN 路由为热更新值，因此无需区分 applied/desired。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkSettingsView {
    pub applied: DataPlaneSettings,
    pub desired: DataPlaneSettings,
    pub server_routes: Vec<String>,
    pub restart_required: bool,
    pub psk_configured: bool,
}

impl Default for NetworkSettings {
    fn default() -> Self {
        Self {
            mode: NetworkMtuMode::Fixed,
            default_mtu: DEFAULT_TUN_MTU,
            min_mtu: MIN_TUN_MTU,
            max_mtu: MAX_TUN_MTU,
        }
    }
}

impl NetworkSettings {
    pub fn validate(&self) -> Result<(), String> {
        if MIN_TUN_MTU <= self.min_mtu
            && self.min_mtu <= self.default_mtu
            && self.default_mtu <= self.max_mtu
            && self.max_mtu <= MAX_TUN_MTU
        {
            Ok(())
        } else {
            Err(format!(
                "MTU 必须满足 {MIN_TUN_MTU} <= min_mtu <= default_mtu <= max_mtu <= {MAX_TUN_MTU}"
            ))
        }
    }
}

/// 更新网络参数请求（PUT /api/v1/admin/network/settings）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateNetworkSettingsRequest {
    pub desired: DataPlaneSettings,
    pub server_routes: Vec<String>,
}

impl VpnBaseSettings {
    pub fn validate(&self) -> Result<(), String> {
        let subnet: ipnet::Ipv4Net = self
            .vpn_subnet
            .parse()
            .map_err(|_| "vpn_subnet 必须是合法 IPv4 CIDR".to_string())?;
        if subnet.prefix_len() > 30 {
            return Err("vpn_subnet 必须至少容纳服务端和一个客户端".to_string());
        }
        validate_endpoint("vpn_endpoint", &self.vpn_endpoint)?;
        if self.vpn_listen_port == 0 {
            return Err("vpn_listen_port 必须大于 0".to_string());
        }
        if !matches!(
            self.wg_backend.as_str(),
            "noop" | "kernel" | "userspace" | "auto"
        ) {
            return Err("wg_backend 必须是 noop、kernel、userspace 或 auto".to_string());
        }
        if self.wg_interface.trim().is_empty()
            || self.wg_interface.len() > 15
            || !self.wg_interface.chars().any(|c| c.is_ascii_alphanumeric())
            || !self
                .wg_interface
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        {
            return Err("wg_interface 必须是 1..=15 位字母、数字、_、- 或 .".to_string());
        }
        Ok(())
    }
}

impl ObfsNetworkSettings {
    pub fn validate(&self) -> Result<(), String> {
        let bind: std::net::SocketAddr = self
            .bind_addr
            .parse()
            .map_err(|_| "obfs.bind_addr 必须是 IPv4 socket 地址".to_string())?;
        if !bind.is_ipv4() {
            return Err("obfs.bind_addr 当前仅支持 IPv4".to_string());
        }
        if bind.port() == 0 {
            return Err("obfs.bind_addr 端口必须大于 0".to_string());
        }
        validate_endpoint("obfs.public_endpoint", &self.public_endpoint)?;
        if !(576..=9000).contains(&self.path_mtu) {
            return Err("obfs.path_mtu 必须在 576..=9000".to_string());
        }
        Ok(())
    }
}

impl DnsNetworkSettings {
    pub fn normalized(mut self) -> Result<Self, String> {
        self.validate()?;
        self.split_domains = self
            .split_domains
            .into_iter()
            .map(|domain| normalize_dns_domain(&domain))
            .collect::<Result<Vec<_>, _>>()?;
        for rule in &mut self.forward_rules {
            rule.domain = normalize_dns_domain(&rule.domain)?;
        }
        for record in &mut self.static_records {
            record.name = normalize_dns_domain(&record.name)?;
        }
        Ok(self)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.mode != ClientDnsMode::Disabled && self.default_upstreams.is_empty() {
            return Err("启用 DNS 时必须配置至少一个默认上游".to_string());
        }
        if self.mode == ClientDnsMode::Split && self.split_domains.is_empty() {
            return Err("分流 DNS 模式必须配置至少一个分流域名".to_string());
        }
        if self.default_upstreams.len() > 8 {
            return Err("默认 DNS 上游不能超过 8 个".to_string());
        }
        for upstream in &self.default_upstreams {
            validate_dns_upstream(upstream)?;
        }
        if self.split_domains.len() > 64 {
            return Err("分流域名不能超过 64 个".to_string());
        }
        validate_unique_domains("分流域名", self.split_domains.iter().map(String::as_str))?;
        if self.forward_rules.len() > 64 {
            return Err("DNS 转发规则不能超过 64 条".to_string());
        }
        validate_unique_domains(
            "DNS 转发规则域名",
            self.forward_rules.iter().map(|rule| rule.domain.as_str()),
        )?;
        for rule in &self.forward_rules {
            if rule.upstreams.is_empty() || rule.upstreams.len() > 8 {
                return Err(format!("域名 {} 的上游数量必须在 1..=8", rule.domain));
            }
            for upstream in &rule.upstreams {
                validate_dns_upstream(upstream)?;
            }
        }
        if self.static_records.len() > 256 {
            return Err("DNS 静态记录不能超过 256 条".to_string());
        }
        validate_unique_domains(
            "DNS 静态记录",
            self.static_records
                .iter()
                .map(|record| record.name.as_str()),
        )?;
        for record in &self.static_records {
            if !(30..=86_400).contains(&record.ttl) {
                return Err(format!("静态记录 {} 的 TTL 必须在 30..=86400", record.name));
            }
        }
        Ok(())
    }
}

impl DataPlaneSettings {
    pub fn validate(&self) -> Result<(), String> {
        self.vpn.validate()?;
        self.obfs.validate()?;
        self.mtu.validate()?;
        self.dns.validate()?;
        if self.obfs.enabled && self.vpn.wg_backend == "noop" {
            return Err("启用 UDP 混淆时 wg_backend 不能为 noop".to_string());
        }
        let safe = obfs_transport_safe_mtu(self.obfs.mode, self.obfs.path_mtu);
        if self.obfs.enabled {
            let (field, value) = match self.mtu.mode {
                NetworkMtuMode::Fixed => ("default_mtu", self.mtu.default_mtu),
                NetworkMtuMode::Auto => ("min_mtu", self.mtu.min_mtu),
            };
            if value > safe {
                return Err(format!(
                    "obfs.path_mtu 的安全内层 MTU 上限为 {safe}，{field}={value} 超过上限"
                ));
            }
        }
        Ok(())
    }
}

fn validate_dns_upstream(upstream: &str) -> Result<(), String> {
    let address: std::net::SocketAddr = upstream
        .parse()
        .map_err(|_| format!("DNS 上游必须是 IP:端口，当前为 {upstream}"))?;
    if !address.is_ipv4() || address.port() != 53 {
        return Err(format!("DNS 上游必须是 IPv4:53：{upstream}"));
    }
    Ok(())
}

fn validate_unique_domains<'a>(
    field: &str,
    domains: impl Iterator<Item = &'a str>,
) -> Result<(), String> {
    let mut seen = std::collections::HashSet::new();
    for domain in domains {
        let normalized = normalize_dns_domain(domain)?;
        if !seen.insert(normalized) {
            return Err(format!("{field}存在重复域名：{domain}"));
        }
    }
    Ok(())
}

pub fn normalize_dns_domain(domain: &str) -> Result<String, String> {
    let domain = domain.trim().trim_end_matches('.').to_ascii_lowercase();
    if domain.is_empty()
        || domain.len() > 253
        || !domain.contains('.')
        || domain.contains('*')
        || !valid_hostname(&domain)
    {
        return Err(format!("DNS 域名必须是完整域名且不能包含通配符：{domain}"));
    }
    Ok(domain)
}

fn validate_endpoint(name: &str, endpoint: &str) -> Result<(), String> {
    let (host, port) = endpoint
        .rsplit_once(':')
        .ok_or_else(|| format!("{name} 必须是 host:port"))?;
    let valid_host = host.parse::<Ipv4Addr>().is_ok() || valid_hostname(host);
    if !valid_host || port.parse::<u16>().ok().filter(|port| *port > 0).is_none() {
        return Err(format!("{name} 必须是有效的 IPv4/域名 host:port"));
    }
    Ok(())
}

fn valid_hostname(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '-')
        })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemInfo {
    pub version: String,
    pub vpn_subnet: String,
    pub server_public_key: String,
    pub server_endpoint: String,
    pub listen_port: u16,
    pub started_at: i64,
    /// 服务端配置的 LAN 网段（服务端作网关下发给客户端的 allowed_routes）。
    #[serde(default)]
    pub server_routes: Vec<String>,
}

/// 更新服务端 LAN 网段请求（PUT /api/v1/admin/system/routes）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateServerRoutesRequest {
    /// LAN 网段 CIDR 列表（空数组表示清空）。
    pub routes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailNotificationSettings {
    pub enabled: bool,
    pub smtp_host: Option<String>,
    pub smtp_port: u16,
    pub smtp_username: Option<String>,
    pub smtp_password_set: bool,
    pub from: Option<String>,
    pub recipients: Vec<String>,
    #[serde(default = "default_quiet_minutes")]
    pub quiet_minutes: u32,
    #[serde(default = "default_true")]
    pub gateway_offline_enabled: bool,
    #[serde(default = "default_true")]
    pub gateway_recovered_enabled: bool,
    #[serde(default)]
    pub webhook: HttpNotificationChannelSettings,
    #[serde(default)]
    pub feishu: HttpNotificationChannelSettings,
    #[serde(default)]
    pub dingtalk: HttpNotificationChannelSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateEmailNotificationSettingsRequest {
    pub enabled: bool,
    pub smtp_host: Option<String>,
    pub smtp_port: u16,
    pub smtp_username: Option<String>,
    /// 留空或不传表示不修改当前密码；传空字符串表示清空密码。
    pub smtp_password: Option<String>,
    pub from: Option<String>,
    pub recipients: Vec<String>,
    #[serde(default = "default_quiet_minutes")]
    pub quiet_minutes: u32,
    #[serde(default = "default_true")]
    pub gateway_offline_enabled: bool,
    #[serde(default = "default_true")]
    pub gateway_recovered_enabled: bool,
    #[serde(default)]
    pub webhook: HttpNotificationChannelSettings,
    #[serde(default)]
    pub feishu: HttpNotificationChannelSettings,
    #[serde(default)]
    pub dingtalk: HttpNotificationChannelSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HttpNotificationChannelSettings {
    pub enabled: bool,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestEmailNotificationRequest {
    pub recipient: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationEventView {
    pub id: String,
    pub event_type: String,
    pub channel: String,
    pub target: String,
    pub status: String,
    pub subject: String,
    pub error: Option<String>,
    pub metadata: Option<String>,
    pub created_at: i64,
    pub sent_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NotificationEventQuery {
    pub event_type: Option<String>,
    pub status: Option<String>,
    pub limit: Option<u32>,
}

fn default_quiet_minutes() -> u32 {
    30
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn obfs_safe_mtu_matches_both_transport_modes() {
        assert_eq!(obfs_transport_safe_mtu(ObfsMode::ParanoidV1, 1500), 1392);
        assert_eq!(obfs_transport_safe_mtu(ObfsMode::LowOverheadV1, 1500), 1440);
        assert_eq!(obfs_transport_safe_mtu(ObfsMode::LowOverheadV1, 1200), 1136);
    }

    #[test]
    fn endpoint_and_interface_validation_rejects_runtime_failures() {
        let mut vpn = VpnBaseSettings {
            vpn_subnet: "10.8.0.0/24".into(),
            vpn_listen_port: 51820,
            vpn_endpoint: "vpn.example.com:51820".into(),
            wg_backend: "kernel".into(),
            wg_interface: "wg0".into(),
        };
        assert!(vpn.validate().is_ok());
        vpn.vpn_endpoint = "http://vpn.example.com:51820".into();
        assert!(vpn.validate().is_err());
        vpn.vpn_endpoint = "vpn.example.com:51820".into();
        vpn.wg_interface = "..".into();
        assert!(vpn.validate().is_err());

        let obfs = ObfsNetworkSettings {
            enabled: true,
            mode: ObfsMode::LowOverheadV1,
            bind_addr: "0.0.0.0:0".into(),
            public_endpoint: "vpn.example.com:47358".into(),
            path_mtu: 1500,
        };
        assert!(obfs.validate().is_err());
    }

    #[test]
    fn dns_validation_normalizes_domains_and_rejects_unsafe_inputs() {
        assert_eq!(
            normalize_dns_domain("API.Corp.Example.COM.").unwrap(),
            "api.corp.example.com"
        );
        assert!(normalize_dns_domain("*.example.com").is_err());
        assert!(normalize_dns_domain("localhost").is_err());

        let valid = DnsNetworkSettings {
            mode: ClientDnsMode::Split,
            split_domains: vec!["corp.example.com".into()],
            default_upstreams: vec!["223.5.5.5:53".into()],
            forward_rules: vec![DnsForwardRule {
                domain: "internal.example.com".into(),
                upstreams: vec!["10.0.0.53:53".into()],
            }],
            static_records: vec![DnsStaticRecord {
                name: "service.internal.example.com".into(),
                address: "10.0.0.10".parse().unwrap(),
                ttl: 300,
            }],
        };
        assert!(valid.validate().is_ok());
        let mut invalid = valid;
        invalid.default_upstreams = vec!["223.5.5.5:5353".into()];
        assert!(invalid.validate().is_err());
    }
}
