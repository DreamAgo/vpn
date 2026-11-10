//! Independent gateway runner. WireGuard terminates here, while routing policy
//! sends decrypted traffic through the controller's encrypted transit link.

use std::time::Duration;
use tokio::{net::UdpSocket, sync::watch, time::Instant};

const PROBE_MAGIC: &[u8] = b"YILIAN01";
const PROBE_LEN: usize = 24;

/// Echo fixed-size probes without amplification or unbounded source state.
/// Remote gateways provide a readiness deadline. The local gateway passes None.
pub async fn run_probe(
    socket: UdpSocket,
    ready_until: Option<watch::Receiver<Option<Instant>>>,
) -> anyhow::Result<()> {
    let mut buf = [0; PROBE_LEN + 1];
    let mut rate_window = Instant::now();
    let mut replies = 0;
    loop {
        let (size, source) = socket.recv_from(&mut buf).await?;
        if size != PROBE_LEN || !buf.starts_with(PROBE_MAGIC) {
            continue;
        }
        let now = Instant::now();
        if ready_until
            .as_ref()
            .is_some_and(|state| state.borrow().is_none_or(|until| until <= now))
        {
            continue;
        }
        if now.duration_since(rate_window) >= Duration::from_secs(1) {
            rate_window = now;
            replies = 0;
        }
        if replies < 100 {
            replies += 1;
            let _ = socket.send_to(&buf[..size], source).await;
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub async fn run_from_env() -> anyhow::Result<()> {
    anyhow::bail!("VPN 网关模式需要 Linux、WireGuard 和 nftables")
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;

    use std::{
        cmp::Reverse,
        collections::{BTreeMap, BTreeSet},
        env,
        fmt::Write,
        fs::{File, OpenOptions},
        net::{Ipv4Addr, SocketAddr},
        os::unix::fs::OpenOptionsExt,
        process::Stdio,
    };

    use anyhow::{anyhow, bail, ensure, Context, Result};
    use base64::{engine::general_purpose::STANDARD, Engine};
    use ipnet::Ipv4Net;
    use reqwest::{Client, Url};
    use serde::{Deserialize, Serialize};
    use tokio::{io::AsyncWriteExt, process::Command, time::MissedTickBehavior};
    use vpn_api_types::{peer::ObfsMode, system::ObfsNetworkSettings};
    use vpn_wireguard::{KernelWireGuardControl, WgMode, WgPeerConfig};
    use zeroize::Zeroizing;

    use crate::{config::ServerConfig, shutdown::shutdown_signal, udp_obfs::UdpObfsServer};

    const TABLE: &str = "yilian_vpn_gateway";
    const UPLINK_TABLE: &str = "47359";
    const DOWNLINK_TABLE: &str = "47360";
    const MAX_TTL: Duration = Duration::from_secs(10);
    // Leave time for the netlink transaction to take effect before the
    // controller can grant the same addresses to another gateway.
    const APPLY_MARGIN: Duration = Duration::from_millis(1500);
    const COMMAND_TIMEOUT: Duration = Duration::from_secs(1);
    const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

    struct Config {
        id: String,
        sync_url: Url,
        client: Client,
        token: Zeroizing<String>,
        private_key: Zeroizing<String>,
        public_key: String,
        client_iface: String,
        transit_iface: String,
        transit_health_addr: Ipv4Addr,
        subnet: Ipv4Net,
        mode: WgMode,
        listen_port: u16,
        probe_addr: SocketAddr,
        obfs: Option<crate::config::ObfsConfig>,
    }

    impl Config {
        fn from_env() -> Result<Self> {
            let id = required("VPN_GATEWAY_ID")?;
            ensure!(
                id != "local"
                    && id.len() <= 32
                    && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'),
                "VPN_GATEWAY_ID 只能包含字母、数字和短横线，最多 32 字节，不能使用 local"
            );
            let mut sync_url = Url::parse(&required("VPN_GATEWAY_CONTROL_URL")?)
                .context("VPN_GATEWAY_CONTROL_URL 格式无效")?;
            ensure!(
                sync_url.scheme() == "https"
                    && sync_url.path() == "/"
                    && sync_url.username().is_empty()
                    && sync_url.password().is_none()
                    && sync_url.query().is_none()
                    && sync_url.fragment().is_none(),
                "VPN_GATEWAY_CONTROL_URL 必须是 HTTPS 源地址"
            );
            sync_url.set_path(&format!("/api/v1/cluster/agents/{id}/sync"));

            let token = read_secret("VPN_GATEWAY_TOKEN_FILE")?;
            ensure!(
                token.len() == 64
                    && token
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                "网关令牌必须是 32 字节小写十六进制"
            );
            let private_key = read_secret("VPN_GATEWAY_WG_KEY_FILE")?;
            let public_key = vpn_wireguard::public_key_from_private(&private_key)?;
            let client_iface = env::var("VPN_WG_INTERFACE").unwrap_or_else(|_| "yilian-wg".into());
            let transit_iface = required("VPN_GATEWAY_TRANSIT_INTERFACE")?;
            let transit_health_addr = required("VPN_GATEWAY_TRANSIT_HEALTH_ADDR")?
                .parse::<Ipv4Addr>()
                .context("VPN_GATEWAY_TRANSIT_HEALTH_ADDR 必须是控制面互连接口的 IPv4 地址")?;
            ensure!(
                valid_interface(&client_iface)
                    && valid_interface(&transit_iface)
                    && client_iface != transit_iface,
                "客户端和互连接口名必须有效且互不相同"
            );
            let subnet = env::var("VPN_SUBNET")
                .unwrap_or_else(|_| "10.8.0.0/24".into())
                .parse::<Ipv4Net>()
                .context("VPN_SUBNET 必须是 IPv4 CIDR")?;
            let mode = match env::var("VPN_WG_BACKEND").as_deref() {
                Ok("kernel") | Err(env::VarError::NotPresent) => WgMode::Kernel,
                Ok("userspace") => WgMode::Userspace,
                _ => bail!("网关模式的 VPN_WG_BACKEND 必须是 kernel 或 userspace"),
            };
            let listen_port = env::var("VPN_LISTEN_PORT")
                .unwrap_or_else(|_| "51820".into())
                .parse::<u16>()
                .context("VPN_LISTEN_PORT 必须是端口号")?;
            ensure!(listen_port != 0, "VPN_LISTEN_PORT 不能为 0");
            let probe_addr = env::var("VPN_GATEWAY_PROBE_BIND_ADDR")
                .unwrap_or_else(|_| "0.0.0.0:47359".into())
                .parse::<SocketAddr>()
                .context("VPN_GATEWAY_PROBE_BIND_ADDR 必须是 IP:端口")?;
            ensure!(
                probe_addr.port() != 0 && probe_addr.port() != listen_port,
                "探测端口不能为 0 或与 WireGuard 端口相同"
            );

            let obfs_enabled = match env::var("VPN_OBFS_ENABLED").as_deref() {
                Ok("true" | "1") => true,
                Ok("false" | "0") | Err(env::VarError::NotPresent) => false,
                _ => bail!("VPN_OBFS_ENABLED 必须是 true 或 false"),
            };
            let obfs = if obfs_enabled {
                let mode = match env::var("VPN_OBFS_MODE").as_deref() {
                    Ok("low-overhead-v1") | Err(env::VarError::NotPresent) => {
                        ObfsMode::LowOverheadV1
                    }
                    Ok("paranoid-v1") => ObfsMode::ParanoidV1,
                    _ => bail!("无效的 VPN_OBFS_MODE"),
                };
                let bind_addr = env::var("VPN_OBFS_BIND_ADDR")
                    .unwrap_or_else(|_| "0.0.0.0:47358".into())
                    .parse::<SocketAddr>()
                    .context("VPN_OBFS_BIND_ADDR 必须是 IP:端口")?;
                ensure!(
                    bind_addr.port() != 0
                        && bind_addr.port() != listen_port
                        && bind_addr.port() != probe_addr.port(),
                    "混淆、探测和 WireGuard 端口必须互不相同"
                );
                let path_mtu = env::var("VPN_OBFS_PATH_MTU")
                    .unwrap_or_else(|_| "1500".into())
                    .parse::<u16>()
                    .context("VPN_OBFS_PATH_MTU 格式无效")?;
                ensure!(
                    (576..=9000).contains(&path_mtu),
                    "混淆路径 MTU 必须在 576–9000"
                );
                let mut server = ServerConfig::from_env()?;
                if env::var_os("VPN_GATEWAY_OBFS_PSK_FILE").is_some() {
                    server.data_plane_seed.obfs_psk =
                        Some(read_secret("VPN_GATEWAY_OBFS_PSK_FILE")?);
                }
                server.obfs_config(&ObfsNetworkSettings {
                    enabled: true,
                    mode,
                    bind_addr: bind_addr.to_string(),
                    public_endpoint: required("VPN_OBFS_ENDPOINT")?,
                    path_mtu,
                })?
            } else {
                None
            };

            let mut http = Client::builder()
                .use_rustls_tls()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(3));
            if let Some(path) = env::var_os("VPN_GATEWAY_CA_FILE") {
                let ca = std::fs::read(path).context("读取网关控制面 CA 文件失败")?;
                http = http
                    .tls_built_in_root_certs(false)
                    .add_root_certificate(reqwest::Certificate::from_pem(&ca)?);
            }
            if let Some(path) = env::var_os("VPN_GATEWAY_TLS_IDENTITY_FILE") {
                let identity =
                    Zeroizing::new(std::fs::read(path).context("读取网关 TLS 客户端证书文件失败")?);
                http = http.identity(reqwest::Identity::from_pem(&identity)?);
            }

            Ok(Self {
                id,
                sync_url,
                client: http.build()?,
                token,
                private_key,
                public_key,
                client_iface,
                transit_iface,
                transit_health_addr,
                subnet: subnet.trunc(),
                mode,
                listen_port,
                probe_addr,
                obfs,
            })
        }
    }

    fn required(name: &str) -> Result<String> {
        let value = env::var(name).with_context(|| format!("缺少 {name}"))?;
        ensure!(!value.trim().is_empty(), "{name} 不能为空");
        Ok(value)
    }

    fn read_secret(variable: &str) -> Result<Zeroizing<String>> {
        let value = Zeroizing::new(
            std::fs::read_to_string(required(variable)?)
                .with_context(|| format!("读取 {variable} 指向的文件失败"))?,
        );
        Ok(Zeroizing::new(value.trim().to_owned()))
    }

    fn valid_interface(iface: &str) -> bool {
        !iface.is_empty()
            && iface.len() <= 15
            && iface
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    }

    #[derive(Serialize)]
    struct SyncRequest<'a> {
        applied_epoch: Option<&'a str>,
        server_public_key: &'a str,
        vpn_subnet: String,
        transport_fingerprint: Option<String>,
    }

    #[derive(Deserialize)]
    struct Snapshot {
        epoch: String,
        peers: Vec<PeerLease>,
    }

    #[derive(Deserialize)]
    struct PeerLease {
        public_key: String,
        vpn_ip: Ipv4Addr,
        routed_subnets: Vec<String>,
        ttl_ms: u64,
    }

    #[derive(Clone, PartialEq, Eq)]
    struct PeerRoutes {
        vpn_ip: Ipv4Addr,
        routed_subnets: BTreeSet<Ipv4Net>,
    }

    #[derive(Clone)]
    struct ActivePeer {
        routes: PeerRoutes,
        expires: Instant,
    }

    type PeerMap = BTreeMap<String, ActivePeer>;

    impl Snapshot {
        fn validate(self, config: &Config, started: Instant) -> Result<(String, PeerMap)> {
            ensure!(
                !self.epoch.is_empty() && self.epoch.len() <= 128,
                "网关快照版本无效"
            );
            let mut peers = BTreeMap::new();
            let mut routes = BTreeSet::new();
            for peer in self.peers {
                let expires = started + Duration::from_millis(peer.ttl_ms).min(MAX_TTL);
                ensure!(
                    expires > Instant::now() + APPLY_MARGIN,
                    "网关收到已过期或剩余时间不足的租约"
                );
                ensure!(
                    STANDARD
                        .decode(&peer.public_key)
                        .is_ok_and(|key| key.len() == 32),
                    "快照包含无效的 WireGuard 公钥"
                );
                ensure!(
                    config.subnet.contains(&peer.vpn_ip)
                        && peer.vpn_ip != config.subnet.network()
                        && peer.vpn_ip != config.subnet.broadcast()
                        && config.subnet.hosts().next() != Some(peer.vpn_ip),
                    "快照包含无效的 VPN IP"
                );
                ensure!(
                    routes.insert(Ipv4Net::from(peer.vpn_ip)),
                    "快照包含重复 VPN IP"
                );
                let mut routed_subnets = BTreeSet::new();
                for subnet in peer.routed_subnets {
                    let subnet = subnet
                        .parse::<Ipv4Net>()
                        .context("快照包含无效的站点网段")?
                        .trunc();
                    ensure!(
                        subnet.prefix_len() > 0 && !config.subnet.contains(&subnet),
                        "快照包含无效的站点网段"
                    );
                    if routed_subnets.insert(subnet) {
                        ensure!(routes.insert(subnet), "快照包含重复的站点网段");
                    }
                }
                ensure!(
                    peers
                        .insert(
                            peer.public_key,
                            ActivePeer {
                                routes: PeerRoutes {
                                    vpn_ip: peer.vpn_ip,
                                    routed_subnets
                                },
                                expires,
                            }
                        )
                        .is_none(),
                    "快照包含重复的 WireGuard 公钥"
                );
            }
            Ok((self.epoch, peers))
        }
    }

    async fn fetch_snapshot(config: &Config, applied_epoch: Option<&str>) -> Result<Snapshot> {
        let mut response = config
            .client
            .post(config.sync_url.clone())
            .bearer_auth(config.token.as_str())
            .json(&SyncRequest {
                applied_epoch,
                server_public_key: &config.public_key,
                vpn_subnet: config.subnet.to_string(),
                transport_fingerprint: config.obfs.as_ref().map(|obfs| {
                    crate::cluster::transport_fingerprint(
                        obfs.mode,
                        &obfs.public_endpoint,
                        obfs.path_mtu,
                        &obfs.psk[..],
                    )
                }),
            })
            .send()
            .await?
            .error_for_status()?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                bytes.len() + chunk.len() <= MAX_RESPONSE_BYTES,
                "网关快照超过大小限制"
            );
            bytes.extend_from_slice(&chunk);
        }
        Ok(serde_json::from_slice(&bytes).context("控制面返回了无效的网关快照")?)
    }

    struct Gateway {
        config: Config,
        peers: PeerMap,
        applied_epoch: Option<String>,
    }

    impl Gateway {
        async fn sync(&mut self, ready: watch::Sender<Option<Instant>>) -> Result<()> {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
            let mut previous_error = None;
            loop {
                interval.tick().await;
                let now = Instant::now();
                if self
                    .peers
                    .values()
                    .any(|peer| peer.expires <= now + APPLY_MARGIN)
                {
                    let peers = self
                        .peers
                        .iter()
                        .filter(|(_, peer)| peer.expires > now + APPLY_MARGIN)
                        .map(|(key, peer)| (key.clone(), peer.clone()))
                        .collect();
                    self.install(peers).await?;
                    self.applied_epoch = None;
                    ready.send_replace(None);
                }
                let result = async {
                    if let Err(error) = self.config.probe_transit().await {
                        ready.send_replace(None);
                        return Err(error);
                    }
                    if let Err(error) = self.config.ensure_uplink_route().await {
                        ready.send_replace(None);
                        return Err(error);
                    }
                    let started = Instant::now();
                    let snapshot =
                        fetch_snapshot(&self.config, self.applied_epoch.as_deref()).await?;
                    let (epoch, peers) = snapshot.validate(&self.config, started)?;
                    Ok::<_, anyhow::Error>((started, epoch, peers))
                }
                .await;
                match result {
                    Ok((started, epoch, peers)) => {
                        self.install(peers).await?;
                        self.applied_epoch = Some(epoch);
                        ready.send_replace(Some(started + MAX_TTL - APPLY_MARGIN));
                        if previous_error.take().is_some() {
                            tracing::info!(gateway = %self.config.id, "网关租约同步恢复");
                        }
                    }
                    Err(error) => {
                        let message = format!("{error:#}");
                        if previous_error.as_ref() != Some(&message) {
                            tracing::warn!(gateway = %self.config.id, error = message, "同步网关租约失败");
                            previous_error = Some(message);
                        }
                    }
                }
            }
        }

        async fn install(&mut self, peers: PeerMap) -> Result<()> {
            let changed = |key: &str, previous: &ActivePeer| {
                peers
                    .get(key)
                    .is_none_or(|next| next.routes != previous.routes)
            };
            if self.peers.len() != peers.len()
                || self.peers.iter().any(|(key, peer)| changed(key, peer))
            {
                // Fence all traffic before changing cryptokey routing.
                // In particular, a shorter overlapping prefix must never
                // temporarily allow a removed peer's traffic.
                apply_fence(&self.config, &PeerMap::new()).await?;
                for (key, peer) in &self.peers {
                    if changed(key, peer) {
                        run_command(Command::new("wg").args([
                            "set",
                            &self.config.client_iface,
                            "peer",
                            key,
                            "remove",
                        ]))
                        .await?;
                    }
                }
                let desired_routes = route_set(&peers);
                let previous_routes = route_set(&self.peers);
                for route in previous_routes.difference(&desired_routes) {
                    self.config.client_route("del", &route.to_string()).await?;
                }
                for route in desired_routes.difference(&previous_routes) {
                    self.config
                        .client_route("replace", &route.to_string())
                        .await?;
                }
                for (key, peer) in &peers {
                    if self
                        .peers
                        .get(key)
                        .is_none_or(|old| old.routes != peer.routes)
                    {
                        let allowed = WgPeerConfig {
                            public_key: key.clone(),
                            vpn_ip: peer.routes.vpn_ip,
                            endpoint: None,
                            allowed_subnets: peer
                                .routes
                                .routed_subnets
                                .iter()
                                .map(ToString::to_string)
                                .collect(),
                        }
                        .allowed_ips()
                        .join(",");
                        run_command(Command::new("wg").args([
                            "set",
                            &self.config.client_iface,
                            "peer",
                            key,
                            "allowed-ips",
                            &allowed,
                        ]))
                        .await?;
                    }
                }
            }

            // Remember the exact installed peer set even if time elapsed during
            // netlink updates. In that case the gate remains closed for those peers.
            self.peers = peers;
            apply_fence(&self.config, &self.peers).await?;
            ensure!(
                self.peers
                    .values()
                    .all(|peer| peer.expires > Instant::now() + APPLY_MARGIN),
                "网关应用快照时租约已失效"
            );
            Ok(())
        }
    }

    fn route_set(peers: &PeerMap) -> BTreeSet<Ipv4Net> {
        peers
            .values()
            .flat_map(|peer| peer.routes.routed_subnets.iter().copied())
            .collect()
    }

    async fn run_command(command: &mut Command) -> Result<String> {
        let output = tokio::time::timeout(COMMAND_TIMEOUT, command.kill_on_drop(true).output())
            .await
            .context("网关网络配置命令超时")?
            .context("启动网关网络配置命令失败")?;
        ensure!(
            output.status.success(),
            "网关网络配置命令失败: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    async fn apply_fence(config: &Config, peers: &PeerMap) -> Result<()> {
        let tables = run_command(Command::new("nft").args(["list", "tables", "inet"])).await?;
        let own_table = format!("table inet {TABLE}");
        let exists = tables.lines().any(|line| line.trim() == own_table);
        let script = render_fence(config, peers, exists, Instant::now());
        let mut process = Command::new("nft")
            .args(["--file", "-"])
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .context("启动网关 nft 租约防护失败")?;
        let mut stdin = process.stdin.take().context("打开 nft 输入失败")?;
        let output = tokio::time::timeout(COMMAND_TIMEOUT, async {
            stdin.write_all(script.as_bytes()).await?;
            drop(stdin);
            process.wait_with_output().await
        })
        .await
        .context("安装网关 nft 租约防护超时")?
        .context("安装网关 nft 租约防护失败")?;
        ensure!(
            output.status.success(),
            "网关 nft 租约防护规则失败: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        Ok(())
    }

    fn render_fence(config: &Config, peers: &PeerMap, exists: bool, now: Instant) -> String {
        let mut script = String::new();
        if exists {
            let _ = writeln!(script, "delete table inet {TABLE}");
        }
        let _ = writeln!(script, "table inet {TABLE} {{");
        let mut prefixes: BTreeMap<Reverse<u8>, Vec<(Ipv4Net, u128)>> = BTreeMap::new();
        for peer in peers.values() {
            let ttl = peer
                .expires
                .saturating_duration_since(now + APPLY_MARGIN)
                .as_millis();
            for net in std::iter::once(Ipv4Net::from(peer.routes.vpn_ip))
                .chain(peer.routes.routed_subnets.iter().copied())
            {
                prefixes
                    .entry(Reverse(net.prefix_len()))
                    .or_default()
                    .push((net, ttl));
            }
        }
        // Split by prefix length so the longest prefix wins, matching WG
        // cryptokey routing. An expired /24 must not fall back to a live /16.
        for (Reverse(prefix), routes) in &prefixes {
            let all = routes
                .iter()
                .map(|(route, _)| route.to_string())
                .collect::<Vec<_>>();
            let live = routes
                .iter()
                .filter(|(_, ttl)| *ttl > 0)
                .map(|(route, ttl)| format!("{route} timeout {ttl}ms"))
                .collect::<Vec<_>>();
            let _ = writeln!(
                script,
                "  set routes_{prefix} {{ type ipv4_addr; flags interval; elements = {{ {} }}; }}",
                all.join(", ")
            );
            let _ = writeln!(
                script,
                "  set live_{prefix} {{ type ipv4_addr; flags interval,timeout;"
            );
            if !live.is_empty() {
                let _ = writeln!(script, "    elements = {{ {} }};", live.join(", "));
            }
            let _ = writeln!(script, "  }}");
        }
        for (direction, address) in [("from_client", "saddr"), ("to_client", "daddr")] {
            let _ = writeln!(script, "  chain {direction} {{");
            for Reverse(prefix) in prefixes.keys() {
                let _ = writeln!(script, "    ip {address} @live_{prefix} return");
                let _ = writeln!(script, "    ip {address} @routes_{prefix} drop");
            }
            let _ = writeln!(script, "    drop\n  }}");
        }

        let client = &config.client_iface;
        let transit = &config.transit_iface;
        let _ = writeln!(
            script,
            "  chain forward {{ type filter hook forward priority -10; policy accept;"
        );
        let _ = writeln!(
            script,
            "    iifname \"{client}\" oifname \"{transit}\" jump from_client"
        );
        let _ = writeln!(
            script,
            "    iifname \"{transit}\" oifname \"{client}\" jump to_client"
        );
        let _ = writeln!(
            script,
            "    iifname \"{client}\" oifname != \"{transit}\" drop"
        );
        let _ = writeln!(
            script,
            "    oifname \"{client}\" iifname != \"{transit}\" drop"
        );
        let _ = writeln!(script, "  }}");
        let _ = writeln!(
            script,
            "  chain input {{ type filter hook input priority -10; policy accept;"
        );
        let _ = writeln!(script, "    iifname \"{client}\" drop");
        if config.obfs.is_some() {
            let _ = writeln!(
                script,
                "    iifname != \"lo\" udp dport {} drop",
                config.listen_port
            );
        }
        let _ = writeln!(script, "  }}");
        let _ = writeln!(
            script,
            "  chain output {{ type filter hook output priority -10; policy accept;"
        );
        let _ = writeln!(script, "    oifname \"{client}\" drop");
        let _ = writeln!(script, "  }}\n}}");
        script
    }

    impl Config {
        async fn ensure_uplink_route(&self) -> Result<()> {
            run_command(Command::new("ip").args([
                "-4",
                "route",
                "replace",
                "default",
                "dev",
                &self.transit_iface,
                "table",
                UPLINK_TABLE,
            ]))
            .await?;
            Ok(())
        }

        async fn probe_transit(&self) -> Result<()> {
            run_command(Command::new("ping").args([
                "-4",
                "-n",
                "-q",
                "-c",
                "1",
                "-W",
                "0.8",
                "-I",
                &self.transit_iface,
                &self.transit_health_addr.to_string(),
            ]))
            .await
            .context("控制面 WireGuard 互联探测失败")?;
            Ok(())
        }

        async fn client_route(&self, action: &str, subnet: &str) -> Result<()> {
            run_command(Command::new("ip").args([
                "-4",
                "route",
                action,
                subnet,
                "dev",
                &self.client_iface,
                "table",
                DOWNLINK_TABLE,
            ]))
            .await?;
            Ok(())
        }
    }

    async fn existing_ingress_rule(iface: &str, priority: &str, table: &str) -> Result<bool> {
        let rules = run_command(Command::new("ip").args(["-4", "rule", "show"])).await?;
        let number = format!("{priority}:");
        let mut matching = rules
            .lines()
            .filter(|line| line.trim_start().starts_with(&number));
        if let Some(rule) = matching.next() {
            let expected = format!("{number} from all iif {iface} lookup {table}");
            ensure!(
                rule.split_whitespace()
                    .filter(|field| *field != "[detached]")
                    .eq(expected.split_whitespace())
                    && matching.next().is_none(),
                "网关路由规则优先级 {priority} 已被占用"
            );
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn add_ingress_rule(iface: &str, priority: &str, table: &str) -> Result<()> {
        if !existing_ingress_rule(iface, priority, table).await? {
            run_command(Command::new("ip").args([
                "-4", "rule", "add", "priority", priority, "iif", iface, "lookup", table,
            ]))
            .await?;
        }
        Ok(())
    }

    async fn verify_route_table(table: &str, subnet: &str, iface: &str) -> Result<()> {
        let routes =
            run_command(Command::new("ip").args(["-4", "route", "show", "table", table])).await?;
        let expected = format!("{subnet} dev {iface} scope link");
        ensure!(
            routes.split_whitespace().eq(expected.split_whitespace()),
            "网关路由表 {table} 已被占用"
        );
        Ok(())
    }

    async fn verify_reverse_path(iface: &str) -> Result<()> {
        let path = format!("/proc/sys/net/ipv4/conf/{iface}/rp_filter");
        let value = tokio::fs::read_to_string(path)
            .await?
            .trim()
            .parse::<u8>()?;
        let all = tokio::fs::read_to_string("/proc/sys/net/ipv4/conf/all/rp_filter")
            .await?
            .trim()
            .parse::<u8>()?;
        ensure!(
            value.max(all) != 1,
            "接口 {iface} 需要非严格 IPv4 rp_filter"
        );
        Ok(())
    }

    async fn configure_routes(config: &Config) -> Result<()> {
        let peers =
            run_command(Command::new("wg").args(["show", &config.transit_iface, "allowed-ips"]))
                .await?;
        ensure!(
            peers.lines().count() == 1
                && peers.split_whitespace().any(|field| field == "0.0.0.0/0"),
            "网关互连接口必须配置且仅有一个 AllowedIPs 含 0.0.0.0/0 的控制面 peer"
        );
        let forwarding = tokio::fs::read_to_string("/proc/sys/net/ipv4/ip_forward").await?;
        ensure!(forwarding.trim() == "1", "网关必须启用 net.ipv4.ip_forward");
        verify_reverse_path(&config.client_iface).await?;
        verify_reverse_path(&config.transit_iface).await?;

        add_ingress_rule(&config.client_iface, "10200", UPLINK_TABLE).await?;
        add_ingress_rule(&config.transit_iface, "10201", DOWNLINK_TABLE).await?;
        config.ensure_uplink_route().await?;
        verify_route_table(UPLINK_TABLE, "default", &config.transit_iface).await?;
        let subnet = config.subnet.to_string();
        config.client_route("replace", &subnet).await?;
        verify_route_table(DOWNLINK_TABLE, &subnet, &config.client_iface).await?;
        Ok(())
    }

    fn lock_agent() -> Result<File> {
        std::fs::create_dir_all("/run/yilian-vpn-gateway").context("创建网关运行目录失败")?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open("/run/yilian-vpn-gateway/agent.lock")
            .context("打开网关进程锁失败")?;
        lock.try_lock().context("网关进程锁已被占用")?;
        Ok(lock)
    }

    pub(super) async fn run_from_env() -> Result<()> {
        let _lock = lock_agent()?;
        let config = Config::from_env()?;
        // A rule naming another client interface may belong to a previous agent
        // whose WireGuard peers survived. Leave its existing fence intact.
        existing_ingress_rule(&config.client_iface, "10200", UPLINK_TABLE).await?;
        existing_ingress_rule(&config.transit_iface, "10201", DOWNLINK_TABLE).await?;
        // Keep the nft table installed on shutdown. It prevents a surviving
        // wireguard-go or kernel interface from forwarding after process death.
        apply_fence(&config, &PeerMap::new()).await?;
        KernelWireGuardControl::start_without_address(
            &config.client_iface,
            &config.private_key,
            &config.public_key,
            config.listen_port,
            config.mode,
        )
        .await?;
        let client_iface = config.client_iface.clone();
        let run = async {
            configure_routes(&config).await?;
            let probe = UdpSocket::bind(config.probe_addr).await.context("绑定网关探测端口失败")?;
            let obfs = match config.obfs.clone() {
                Some(obfs) => Some(UdpObfsServer::bind(obfs, config.listen_port).await?),
                None => None,
            };
            let (ready_tx, ready_rx) = watch::channel(None);
            tracing::info!(gateway = %config.id, public_key = %config.public_key, "分布式 VPN 网关已启动");
            let mut gateway = Gateway { config, peers: PeerMap::new(), applied_epoch: None };
            let result = tokio::select! {
                result = gateway.sync(ready_tx) => result,
                result = run_probe(probe, Some(ready_rx)) => result,
                result = async {
                    match obfs {
                        Some(obfs) => obfs.run().await,
                        None => std::future::pending().await,
                    }
                } => result,
                _ = shutdown_signal() => Ok(()),
            };
            if let Err(error) = apply_fence(&gateway.config, &PeerMap::new()).await {
                tracing::error!(%error, "清空网关租约失败，内核会按原超时回收");
            }
            result
        }.await;

        if let Err(error) =
            run_command(Command::new("ip").args(["link", "delete", &client_iface])).await
        {
            tracing::error!(%error, "移除网关客户端接口失败");
            return run.and(Err(anyhow!("关闭网关客户端接口失败")));
        }
        run
    }
}

#[cfg(target_os = "linux")]
pub async fn run_from_env() -> anyhow::Result<()> {
    linux::run_from_env().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::timeout;

    #[tokio::test]
    async fn probe_requires_readiness_and_exact_wire_format() {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = socket.local_addr().unwrap();
        let (ready, receive_ready) = watch::channel(None);
        let task = tokio::spawn(run_probe(socket, Some(receive_ready)));
        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        client.connect(addr).await.unwrap();
        let probe = *b"YILIAN01nonce_for_probe!";
        let mut received = [0; PROBE_LEN + 1];

        client.send(&probe).await.unwrap();
        assert!(
            timeout(Duration::from_millis(100), client.recv(&mut received))
                .await
                .is_err()
        );
        ready.send_replace(Some(Instant::now() + Duration::from_secs(2)));
        let mut oversize = probe.to_vec();
        oversize.push(0);
        client.send(&oversize).await.unwrap();
        client.send(&probe).await.unwrap();
        let size = timeout(Duration::from_secs(1), client.recv(&mut received))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&received[..size], &probe);
        assert!(
            timeout(Duration::from_millis(100), client.recv(&mut received))
                .await
                .is_err()
        );
        task.abort();
    }
}
