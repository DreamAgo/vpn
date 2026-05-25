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
    /// 该 peer 背后路由的额外 LAN 网段（CIDR），与 vpn_ip/32 一并作为 allowed-ips。
    pub allowed_subnets: Vec<String>,
}

impl WgPeerConfig {
    /// 服务端为该 peer 设置的完整 allowed-ips 列表：`<vpn_ip>/32` + 各 LAN 网段。
    pub fn allowed_ips(&self) -> Vec<String> {
        let mut v = vec![format!("{}/32", self.vpn_ip)];
        v.extend(self.allowed_subnets.iter().cloned());
        v
    }
}

/// 渲染标准客户端 `.conf` 文本（Story 4.7 配置下载）。
///
/// `client_private_key` 通常由客户端持有，服务端不存私钥，故此处放占位符
/// 供用户手动填入；若调用方有私钥也可直接传入。
///
/// `allowed_ips` 控制客户端把哪些目的网段导入隧道（分隧道）：
/// 传入 VPN 子网 + 各站点 LAN 网段即可只路由这些，普通上网走本地；
/// 传 `["0.0.0.0/0"]` 则为全隧道。
#[allow(clippy::too_many_arguments)]
pub fn render_client_config(
    client_private_key: &str,
    client_vpn_ip: Ipv4Addr,
    subnet_prefix_len: u8,
    dns: &str,
    server_public_key: &str,
    server_endpoint: &str,
    persistent_keepalive: u16,
    allowed_ips: &[String],
) -> String {
    let allowed = if allowed_ips.is_empty() {
        "0.0.0.0/0".to_string()
    } else {
        allowed_ips.join(", ")
    };
    format!(
        "[Interface]\n\
         PrivateKey = {client_private_key}\n\
         Address = {client_vpn_ip}/{subnet_prefix_len}\n\
         DNS = {dns}\n\
         \n\
         [Peer]\n\
         PublicKey = {server_public_key}\n\
         Endpoint = {server_endpoint}\n\
         AllowedIPs = {allowed}\n\
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
            &["10.8.0.0/24".to_string(), "192.168.10.0/24".to_string()],
        );
        assert!(conf.contains("[Interface]"));
        assert!(conf.contains("Address = 10.8.0.5/24"));
        assert!(conf.contains("[Peer]"));
        assert!(conf.contains("PublicKey = SERVER_PUB_KEY"));
        assert!(conf.contains("Endpoint = vpn.example.com:51820"));
        assert!(conf.contains("AllowedIPs = 10.8.0.0/24, 192.168.10.0/24"));
        assert!(conf.contains("PersistentKeepalive = 25"));
    }

    #[test]
    fn empty_allowed_ips_defaults_to_full_tunnel() {
        let conf = render_client_config(
            "K",
            "10.8.0.5".parse().unwrap(),
            24,
            "10.8.0.1",
            "S",
            "h:51820",
            25,
            &[],
        );
        assert!(conf.contains("AllowedIPs = 0.0.0.0/0"));
    }

    #[test]
    fn peer_config_allowed_ips_includes_subnets() {
        let cfg = WgPeerConfig {
            public_key: "PK".to_string(),
            vpn_ip: "10.8.0.2".parse().unwrap(),
            endpoint: None,
            allowed_subnets: vec!["192.168.10.0/24".to_string()],
        };
        assert_eq!(cfg.allowed_ips(), vec!["10.8.0.2/32", "192.168.10.0/24"]);
    }
}
