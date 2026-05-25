//! VPN 节点（peer）相关 DTO（Epic 4）。
//!
//! 前后端 + CLI 共享契约：
//! - 客户端用 `PeerRegisterRequest` 注册，得到 `PeerRegisterResponse`（含分配的 VPN IP 与服务端信息）
//! - 客户端 daemon 周期性 `PeerHeartbeatRequest` 上报在线
//! - admin 后台用 `PeerDto` 展示节点列表/状态

use serde::{Deserialize, Serialize};

/// 注册节点请求（POST /api/v1/peers/register）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerRegisterRequest {
    /// 客户端生成的 WireGuard 公钥（base64）
    pub wg_public_key: String,
    /// 设备名（用户可读，如 "Lin's MacBook"）
    pub device_name: String,
    /// 可选：操作系统信息（如 "macOS 15.4 arm64"）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os_info: Option<String>,
}

/// 注册节点响应：客户端据此组装本地 WireGuard 隧道。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerRegisterResponse {
    /// 分配给本设备的 VPN IP（静态绑定，重连不变）
    pub vpn_ip: String,
    /// 服务端 WireGuard 公钥（base64）
    pub server_public_key: String,
    /// 服务端 WireGuard endpoint（host:port）
    pub server_endpoint: String,
    /// VPN 子网（CIDR，如 10.8.0.0/24）
    pub vpn_subnet: String,
}

/// 心跳请求（POST /api/v1/peers/heartbeat），每 30s 一次。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerHeartbeatRequest {
    /// 客户端当前出口 endpoint（IP:port），用于漫游与展示；可空
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
}

/// admin 后台展示的节点视图。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerDto {
    pub id: String,
    pub user_id: String,
    pub device_name: String,
    pub wg_public_key: String,
    pub vpn_ip: String,
    pub endpoint: Option<String>,
    pub os_info: Option<String>,
    pub last_seen_at: Option<i64>,
    /// "online" | "offline" | "deleted"
    pub status: String,
    pub created_at: i64,
}
