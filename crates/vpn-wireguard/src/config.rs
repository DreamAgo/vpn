//! WireGuard peer 配置类型 + 客户端 .conf 渲染。

use std::net::Ipv4Addr;

/// 单个 peer 的服务端侧配置（用于 configure_peer）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WgPeerConfig {
    /// peer 公钥（base64）
    pub public_key: String,
    /// 分配给该 peer 的 VPN IP（服务端以 /32 加入 AllowedIPs）
    pub vpn_ip: Ipv4Addr,
    /// 可选：peer 最近上报的 endpoint（IP:port），用于漫游
    pub endpoint: Option<String>,
}

/// 渲染标准客户端 `.conf` 文本（Story 4.7 配置下载）。
///
/// `client_private_key` 通常由客户端持有，服务端不存私钥，故此处放占位符
/// 供用户手动填入；若调用方有私钥也可直接传入。
#[allow(clippy::too_many_arguments)]
pub fn render_client_config(
    client_private_key: &str,
    client_vpn_ip: Ipv4Addr,
    subnet_prefix_len: u8,
    dns: &str,
    server_public_key: &str,
    server_endpoint: &str,
    persistent_keepalive: u16,
) -> String {
    format!(
        "[Interface]\n\
         PrivateKey = {client_private_key}\n\
         Address = {client_vpn_ip}/{subnet_prefix_len}\n\
         DNS = {dns}\n\
         \n\
         [Peer]\n\
         PublicKey = {server_public_key}\n\
         Endpoint = {server_endpoint}\n\
         AllowedIPs = 0.0.0.0/0\n\
         PersistentKeepalive = {persistent_keepalive}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_valid_conf() {
        let conf = render_client_config(
            "<PRIVATE_KEY>",
            "10.8.0.5".parse().unwrap(),
            24,
            "10.8.0.1",
            "SERVER_PUB_KEY",
            "vpn.example.com:51820",
            25,
        );
        assert!(conf.contains("[Interface]"));
        assert!(conf.contains("Address = 10.8.0.5/24"));
        assert!(conf.contains("[Peer]"));
        assert!(conf.contains("PublicKey = SERVER_PUB_KEY"));
        assert!(conf.contains("Endpoint = vpn.example.com:51820"));
        assert!(conf.contains("PersistentKeepalive = 25"));
    }
}
