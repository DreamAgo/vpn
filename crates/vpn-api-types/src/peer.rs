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
    /// 可选：该节点背后路由的 LAN 网段（站点内网 CIDR，如 "192.168.10.0/24"）。
    /// 作为站点网关时声明，服务端会把这些网段路由到本节点。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub routed_subnets: Vec<String>,
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
    /// 客户端应路由进隧道的网段（AllowedIPs）：VPN 子网 + 其他站点的 LAN 网段。
    /// 客户端据此实现分隧道（只把这些网段导入 VPN，普通上网走本地）。
    #[serde(default)]
    pub allowed_routes: Vec<String>,
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
    /// "online" | "offline" | "deleted" | "force_removed"
    pub status: String,
    pub created_at: i64,
    /// 该节点背后路由的 LAN 网段（CIDR 列表）。
    #[serde(default)]
    pub routed_subnets: Vec<String>,
}

/// admin peer 列表项：节点信息 + 所属用户（Story 5.5）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminPeerView {
    pub id: String,
    pub user_id: String,
    pub username: String,
    pub email: String,
    pub device_name: String,
    pub wg_public_key: String,
    pub vpn_ip: String,
    pub endpoint: Option<String>,
    pub os_info: Option<String>,
    pub last_seen_at: Option<i64>,
    pub status: String,
    pub created_at: i64,
    #[serde(default)]
    pub routed_subnets: Vec<String>,
}

/// 更新 peer 路由网段请求（PATCH /admin/peers/:id）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdatePeerRoutesRequest {
    /// 该节点背后路由的 LAN 网段（CIDR 列表）。空数组表示清空。
    pub routed_subnets: Vec<String>,
}

/// admin peer 列表查询参数（GET /admin/peers）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminPeerQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_size: Option<u32>,
    /// 模糊匹配 username / device_name
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search: Option<String>,
    /// 按状态筛选
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}
