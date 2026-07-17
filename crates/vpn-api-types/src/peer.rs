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
    /// 可选：客户端版本（如 "0.1.0"，节点健康监控展示用）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_version: Option<String>,
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
    /// 本机 WireGuard 公钥：多终端模式下用于精确定位是哪台终端在打卡。
    /// 旧客户端不携带 → 服务端回退按用户打卡（单终端语义）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wg_public_key: Option<String>,
    /// 上一次心跳请求的往返延迟（毫秒，客户端测量；节点健康监控）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtt_ms: Option<i64>,
    /// 最近 N 次心跳的失败率（百分比 0-100，客户端统计；节点健康监控）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loss_pct: Option<f64>,
}

/// 心跳响应：回带该节点当前应导入隧道的网段。
///
/// 服务端按最新配置(用户组路由 / 全局 server_routes / 站点网关)实时计算;客户端
/// 据此与本地路由表做增量增删，使组/网段变更**无需重连即对在线节点生效**(P1.4)。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PeerHeartbeatResponse {
    #[serde(default)]
    pub allowed_routes: Vec<String>,
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
    /// 本次转为在线的起始时刻（unix ms）；不在线为 None。用于展示“在线时长”。
    #[serde(default)]
    pub online_since: Option<i64>,
    /// 客户端最近上报的心跳往返延迟（毫秒）。
    #[serde(default)]
    pub rtt_ms: Option<i64>,
    /// 客户端最近上报的心跳丢包率（百分比 0-100）。
    #[serde(default)]
    pub loss_pct: Option<f64>,
    /// 客户端版本。
    #[serde(default)]
    pub client_version: Option<String>,
}

/// 节点属性变更记录（OS / IP / Endpoint / 设备名 / 版本；节点健康监控）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerEventView {
    pub id: String,
    pub peer_id: String,
    /// 设备名 / 用户名（peer 或用户已被删除时为 None）。
    pub device_name: Option<String>,
    pub username: Option<String>,
    /// 变更字段：'os_info' | 'endpoint' | 'vpn_ip' | 'device_name' | 'client_version'
    pub field: String,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
    pub created_at: i64,
}

/// 变更记录查询参数（GET /admin/peer-events）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerEventQuery {
    /// 只看某个节点的记录；缺省为全部节点。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_id: Option<String>,
    /// 返回条数（默认 50，最多 200）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
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

#[cfg(test)]
mod tests {
    use super::PeerRegisterRequest;

    #[test]
    fn register_request_ignores_legacy_routed_subnets() {
        let request: PeerRegisterRequest = serde_json::from_value(serde_json::json!({
            "wg_public_key": "pk",
            "device_name": "gateway",
            "routed_subnets": ["192.168.188.0/24"]
        }))
        .unwrap();

        assert_eq!(request.wg_public_key, "pk");
        assert_eq!(request.device_name, "gateway");
        let serialized = serde_json::to_value(request).unwrap();
        assert!(serialized.get("routed_subnets").is_none());
    }
}
