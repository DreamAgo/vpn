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
}
