//! 系统信息 DTO。

use serde::{Deserialize, Serialize};

use crate::peer::ObfsMode;

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
    pub mode: NetworkMtuMode,
    pub default_mtu: u16,
    pub min_mtu: u16,
    pub max_mtu: u16,
}

impl From<UpdateNetworkSettingsRequest> for NetworkSettings {
    fn from(value: UpdateNetworkSettingsRequest) -> Self {
        Self {
            mode: value.mode,
            default_mtu: value.default_mtu,
            min_mtu: value.min_mtu,
            max_mtu: value.max_mtu,
        }
    }
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
}
