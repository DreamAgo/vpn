use crate::{routes::effective_routes, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use ipnet::Ipv4Net;
use serde::Serialize;
use std::net::Ipv4Addr;
use vpn_api_types::{
    peer::PeerRegisterResponse,
    system::{
        normalize_local_route_bypass, obfs_transport_safe_mtu, ClientDnsMode, NetworkMtuMode,
    },
};

#[derive(Serialize)]
pub struct Plan {
    pub address: String,
    pub endpoint: String,
    pub routes: Vec<String>,
    pub mtu: u16,
    pub dns: Option<String>,
}
pub fn key(value: &str) -> Result<[u8; 32]> {
    let bytes = STANDARD.decode(value).map_err(|_| "Invalid base64 key")?;
    bytes.try_into().map_err(|_| "Key must be 32 bytes".into())
}
pub fn plan(config: &PeerRegisterResponse, local: &[String]) -> Result<Plan> {
    if key(&config.server_public_key)? == [0; 32] {
        return Err("Invalid server key".into());
    }
    let vpn: Ipv4Net = config
        .vpn_subnet
        .parse()
        .map_err(|_| "Invalid VPN subnet")?;
    let address: Ipv4Addr = config.vpn_ip.parse().map_err(|_| "Invalid VPN IP")?;
    if vpn.prefix_len() > 30
        || vpn.prefix_len() == 0
        || !vpn.contains(&address)
        || address == vpn.network()
        || address == vpn.broadcast()
    {
        return Err("Invalid VPN address allocation".into());
    }
    let local: Vec<Ipv4Addr> = local
        .iter()
        .map(|s| s.parse().map_err(|_| "Invalid physical address".into()))
        .collect::<Result<_>>()?;
    let rules = normalize_local_route_bypass(&config.local_route_bypass)?;
    let mut excluded = Vec::new();
    for rule in rules {
        let matches = rule.local_subnets.iter().any(|s| {
            s.parse::<Ipv4Net>()
                .is_ok_and(|n| local.iter().any(|a| n.contains(a)))
        });
        if matches {
            for value in rule.excluded_routes {
                excluded.push(value.parse().map_err(|_| "Invalid exclusion")?);
            }
        }
    }
    let routes =
        effective_routes(&config.allowed_routes, vpn, &excluded).map_err(|e| e.to_string())?;
    let settings = config.network_settings.clone().unwrap_or_default();
    settings.validate()?;
    let mut mtu = settings.default_mtu;
    let endpoint = if let Some(t) = &config.transport {
        if t.protocol != "obfs-v1" || !(576..=9000).contains(&t.path_mtu) {
            return Err("Invalid obfs transport".into());
        }
        key(&t.psk)?;
        let safe = obfs_transport_safe_mtu(t.mode, t.path_mtu);
        if settings.mode == NetworkMtuMode::Auto {
            mtu = safe.clamp(settings.min_mtu, settings.max_mtu);
        }
        if mtu > safe {
            return Err("MTU exceeds obfs path capacity".into());
        }
        &t.endpoint
    } else {
        &config.server_endpoint
    };
    let (host, port) = endpoint.rsplit_once(':').ok_or("Invalid endpoint")?;
    if host.is_empty()
        || host.contains(['/', '@', ' ', '\n'])
        || port.parse::<u16>().ok().filter(|p| *p > 0).is_none()
    {
        return Err("Invalid endpoint".into());
    }
    let dns = if let Some(dns) = &config.dns {
        if dns.mode == ClientDnsMode::Disabled {
            None
        } else {
            let ip: Ipv4Addr = dns.server.parse().map_err(|_| "Invalid DNS address")?;
            if u32::from(ip) != u32::from(vpn.network()) + 1 {
                return Err("DNS must be VPN gateway".into());
            }
            Some(ip.to_string())
        }
    } else {
        None
    };
    Ok(Plan {
        address: address.to_string(),
        endpoint: endpoint.clone(),
        routes: routes.into_iter().map(|r| r.to_string()).collect(),
        mtu,
        dns,
    })
}
