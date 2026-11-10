//! Controller-side routing and source fencing for preprovisioned WireGuard
//! transit interfaces. All remote client traffic traverses the central ACL.
use std::{collections::BTreeMap, net::Ipv4Addr, process::Stdio, time::Duration};

use ipnet::Ipv4Net;
use tokio::{io::AsyncWriteExt, process::Command};
use vpn_core::{AppError, Result};

const TABLE: &str = "yilian_cluster_transit";
const ROUTE_PROTOCOL: &str = "186";

pub struct RouteLease {
    pub vpn_ip: String,
    pub public_key: String,
    pub interface: String,
    pub routes: Vec<String>,
    pub issued_until: i64,
}

pub struct TransitLink {
    pub interface: String,
    pub peer_ip: Ipv4Addr,
    pub local_ip: Ipv4Addr,
}

pub struct Transit {
    links: Vec<TransitLink>,
    local_interface: String,
    subnet: Ipv4Net,
    dns: Ipv4Addr,
    #[cfg(test)]
    mock: bool,
}

impl Transit {
    pub async fn start(
        links: Vec<TransitLink>,
        local_interface: &str,
        subnet: Ipv4Net,
    ) -> Result<Self> {
        for link in &links {
            command("wg", &["show", &link.interface, "public-key"]).await?;
        }
        let dns = subnet
            .hosts()
            .next()
            .ok_or_else(|| AppError::Config("VPN 子网缺少网关地址".into()))?;
        let transit = Self {
            links,
            local_interface: local_interface.to_owned(),
            subnet,
            dns,
            #[cfg(test)]
            mock: false,
        };
        // Fence traffic before manipulating routes left behind after a restart.
        transit.apply(&[], &[]).await?;
        for link in &transit.links {
            command(
                "ip",
                &[
                    "-4",
                    "route",
                    "flush",
                    "dev",
                    &link.interface,
                    "proto",
                    ROUTE_PROTOCOL,
                ],
            )
            .await?;
        }
        Ok(transit)
    }

    #[cfg(test)]
    pub fn mock() -> Self {
        Self {
            links: Vec::new(),
            local_interface: "wg0".into(),
            subnet: "10.8.0.0/24".parse().unwrap(),
            dns: "10.8.0.1".parse().unwrap(),
            mock: true,
        }
    }

    pub fn local_interface(&self) -> &str {
        &self.local_interface
    }

    pub async fn apply(&self, leases: &[RouteLease], reserved: &[Ipv4Net]) -> Result<()> {
        #[cfg(test)]
        if self.mock {
            self.render_fence(leases, reserved, false)?;
            return Ok(());
        }
        let installed = self.installed_routes().await?;
        let mut desired = BTreeMap::new();
        for lease in leases
            .iter()
            .filter(|lease| lease.interface != self.local_interface)
        {
            for net in lease_networks(lease)? {
                if let Some(previous) = desired.insert(net, lease.interface.clone()) {
                    if previous != lease.interface {
                        return Err(AppError::Conflict(format!("多台网关同时声明路由 {net}")));
                    }
                }
            }
        }
        self.apply_fence(leases, reserved).await?;
        let old_routes: Vec<_> = installed
            .iter()
            .filter(|(net, iface)| desired.get(net) != Some(iface))
            .map(|(net, iface)| (*net, iface.clone()))
            .collect();
        for (net, iface) in old_routes {
            let args = [
                "-4",
                "route",
                "del",
                &net.to_string(),
                "dev",
                &iface,
                "proto",
                ROUTE_PROTOCOL,
            ];
            let output = output("ip", &args).await?;
            if !output.status.success()
                && !String::from_utf8_lossy(&output.stderr).contains("No such process")
            {
                return Err(command_error("ip", &output));
            }
        }
        for (net, iface) in desired {
            if installed.get(&net) == Some(&iface) {
                continue;
            }
            command(
                "ip",
                &[
                    "-4",
                    "route",
                    "add",
                    &net.to_string(),
                    "dev",
                    &iface,
                    "proto",
                    ROUTE_PROTOCOL,
                ],
            )
            .await?;
        }
        Ok(())
    }

    async fn installed_routes(&self) -> Result<BTreeMap<Ipv4Net, String>> {
        #[derive(serde::Deserialize)]
        struct Route {
            dst: String,
            dev: Option<String>,
        }
        let routes = output(
            "ip",
            &["-j", "-4", "route", "show", "proto", ROUTE_PROTOCOL],
        )
        .await?;
        if !routes.status.success() {
            return Err(command_error("ip", &routes));
        }
        let routes: Vec<Route> = serde_json::from_slice(&routes.stdout)
            .map_err(|error| AppError::WireGuard(format!("读取集群路由失败：{error}")))?;
        Ok(routes
            .into_iter()
            .filter_map(|route| {
                let iface = route.dev?;
                if !self.links.iter().any(|link| link.interface == iface) {
                    return None;
                }
                // iproute2 emits host routes without an explicit /32.
                let prefix = route.dst.parse::<Ipv4Net>().ok().or_else(|| {
                    route
                        .dst
                        .parse::<Ipv4Addr>()
                        .ok()
                        .and_then(|addr| Ipv4Net::new(addr, 32).ok())
                })?;
                Some((prefix, iface))
            })
            .collect())
    }

    async fn apply_fence(&self, leases: &[RouteLease], reserved: &[Ipv4Net]) -> Result<()> {
        let exists = output("nft", &["list", "table", "inet", TABLE]).await?;
        if !exists.status.success()
            && !String::from_utf8_lossy(&exists.stderr).contains("No such file")
        {
            return Err(command_error("nft", &exists));
        }
        let script = self.render_fence(leases, reserved, exists.status.success())?;
        let mut child = Command::new("nft")
            .args(["-f", "-"])
            .env("LC_ALL", "C")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(io)?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| AppError::WireGuard("nft stdin 不可用".into()))?;
        tokio::time::timeout(Duration::from_secs(3), async move {
            stdin.write_all(script.as_bytes()).await.map_err(io)?;
            drop(stdin);
            let output = child.wait_with_output().await.map_err(io)?;
            if !output.status.success() {
                return Err(command_error("nft", &output));
            }
            Ok(())
        })
        .await
        .map_err(|_| AppError::WireGuard("更新集群 nftables 规则超时".into()))?
    }

    fn render_fence(
        &self,
        leases: &[RouteLease],
        reserved: &[Ipv4Net],
        exists: bool,
    ) -> Result<String> {
        let mut script = String::new();
        if exists {
            script.push_str(&format!("delete table inet {TABLE}\n"));
        }
        script.push_str(&format!("add table inet {TABLE}\n"));
        for chain in ["input", "forward", "output"] {
            script.push_str(&format!("add chain inet {TABLE} {chain} {{ type filter hook {chain} priority -110; policy accept; }}\n"));
        }
        script.push_str(&format!("add chain inet {TABLE} to_edge\n"));
        for link in &self.links {
            let TransitLink {
                interface: iface,
                local_ip,
                peer_ip,
            } = link;
            script.push_str(&format!(
                "add rule inet {TABLE} input iifname \"{iface}\" ip saddr {peer_ip} ip daddr {local_ip} icmp type echo-request accept\n\
                 add rule inet {TABLE} output oifname \"{iface}\" ip saddr {local_ip} ip daddr {peer_ip} icmp type echo-reply accept\n"
            ));
        }
        let now = chrono::Utc::now().timestamp_millis();
        let mut networks = Vec::new();
        for lease in leases {
            let ttl = lease.issued_until.saturating_sub(now).clamp(0, 10_000);
            if ttl == 0 {
                continue;
            }
            for (index, net) in lease_networks(lease)?.into_iter().enumerate() {
                networks.push((net, Some(&lease.interface), ttl, index > 0));
            }
        }
        let active: std::collections::BTreeSet<_> = networks.iter().map(|(net, ..)| *net).collect();
        networks.extend(
            reserved
                .iter()
                .filter(|net| !active.contains(net))
                .map(|net| (*net, None, 0, true)),
        );
        // Match source and destination ownership globally, including when
        // the more-specific prefix belongs to a different transit interface.
        networks.sort_unstable_by_key(|(net, ..)| std::cmp::Reverse(net.prefix_len()));
        let interfaces = std::iter::once(&self.local_interface)
            .chain(self.links.iter().map(|link| &link.interface))
            .map(|iface| format!("\"{iface}\""))
            .collect::<Vec<_>>()
            .join(", ");
        for (index, (net, iface, ttl, is_site)) in networks.into_iter().enumerate() {
            if let Some(iface) = iface {
                let name = format!("lease_{index}");
                let source = if is_site {
                    format!("ip saddr != {}", self.subnet)
                } else {
                    String::new()
                };
                let destination = if is_site {
                    format!("ip daddr != {}", self.subnet)
                } else {
                    String::new()
                };
                let input = if iface == &self.local_interface {
                    String::new()
                } else {
                    format!(
                        "ip daddr {} meta l4proto {{ tcp, udp }} th dport 53",
                        self.dns
                    )
                };
                script.push_str(&format!(
                    "add set inet {TABLE} {name} {{ type ipv4_addr; flags interval,timeout; timeout {ttl}ms; }}\n\
                     add element inet {TABLE} {name} {{ {net} timeout {ttl}ms }}\n\
                     add rule inet {TABLE} input iifname \"{iface}\" ip saddr @{name} {source} {input} accept\n\
                     add rule inet {TABLE} forward iifname \"{iface}\" ip saddr @{name} {source} jump to_edge\n\
                     add rule inet {TABLE} output oifname \"{iface}\" ip daddr @{name} {destination} accept\n\
                     add rule inet {TABLE} to_edge oifname \"{iface}\" ip daddr @{name} {destination} accept\n",
                ));
            }
            script.push_str(&format!(
                "add rule inet {TABLE} input iifname {{ {interfaces} }} ip saddr {net} drop\n\
                 add rule inet {TABLE} forward iifname {{ {interfaces} }} ip saddr {net} drop\n\
                 add rule inet {TABLE} output oifname {{ {interfaces} }} ip daddr {net} drop\n\
                 add rule inet {TABLE} to_edge oifname {{ {interfaces} }} ip daddr {net} drop\n",
            ));
        }
        for iface in std::iter::once(&self.local_interface)
            .chain(self.links.iter().map(|link| &link.interface))
        {
            script.push_str(&format!(
                "add rule inet {TABLE} input iifname \"{iface}\" drop\n\
                add rule inet {TABLE} forward iifname \"{iface}\" drop\n\
                add rule inet {TABLE} to_edge oifname \"{iface}\" drop\n\
                add rule inet {TABLE} output oifname \"{iface}\" drop\n"
            ));
        }
        script.push_str(&format!(
            "add rule inet {TABLE} to_edge accept\nadd rule inet {TABLE} forward jump to_edge\n"
        ));
        Ok(script)
    }
}

pub async fn clear_controller_fence(local_interface: &str) -> Result<()> {
    if !cfg!(target_os = "linux") {
        return Ok(());
    }
    let exists = output("nft", &["list", "table", "inet", TABLE]).await?;
    if !exists.status.success() {
        return if String::from_utf8_lossy(&exists.stderr).contains("No such file") {
            Ok(())
        } else {
            Err(command_error("nft", &exists))
        };
    }
    // Discard old kernel peer state before removing the cluster's last fence.
    if std::path::Path::new("/sys/class/net")
        .join(local_interface)
        .exists()
    {
        command("ip", &["link", "del", local_interface]).await?;
    }
    command("nft", &["delete", "table", "inet", TABLE]).await
}

fn lease_networks(lease: &RouteLease) -> Result<Vec<Ipv4Net>> {
    std::iter::once(format!("{}/32", lease.vpn_ip))
        .chain(lease.routes.iter().cloned())
        .map(|cidr| {
            cidr.parse::<Ipv4Net>()
                .ok()
                .filter(|net| net.prefix_len() > 0)
                .ok_or_else(|| AppError::Config(format!("集群路由无效：{cidr}")))
        })
        .collect()
}

async fn output(program: &str, args: &[&str]) -> Result<std::process::Output> {
    tokio::time::timeout(
        Duration::from_secs(3),
        Command::new(program)
            .args(args)
            .env("LC_ALL", "C")
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| AppError::WireGuard(format!("{program} 命令超时")))?
    .map_err(io)
}

async fn command(program: &str, args: &[&str]) -> Result<()> {
    let result = output(program, args).await?;
    if result.status.success() {
        Ok(())
    } else {
        Err(command_error(program, &result))
    }
}

fn command_error(program: &str, result: &std::process::Output) -> AppError {
    AppError::WireGuard(format!(
        "{program}: {}",
        String::from_utf8_lossy(&result.stderr).trim()
    ))
}

fn io(error: std::io::Error) -> AppError {
    AppError::WireGuard(error.to_string())
}
