//! 系统信息 DTO。

use serde::{Deserialize, Serialize};

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
