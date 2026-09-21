//! 用户态 WireGuard 数据面：boringtun(协议栈) + tun(TUN 设备) + UDP，**零外部依赖**。
//!
//! 不再 shell-out 到 `wg`/`wg-quick`/`wireguard.exe`，整条隧道在进程内完成：
//! - [`boringtun::noise::Tunn`]：握手 + 加解密 + keepalive 状态机；
//! - `tun` crate：跨平台 TUN 设备（Linux `/dev/net/tun`、macOS `utun`、Windows WinTun）；
//! - `net-route`：跨平台路由表(为 allowed_ips 加 `dev <tun>` 路由)；
//! - tokio UDP：与服务端 endpoint 收发密文。
//!
//! 转发循环（单任务 `tokio::select!`）：
//! - 系统路由将需走 VPN 的 IP 包交给 TUN；
//! - TUN 读到 IP 包 → `Tunn::encapsulate` → UDP 送服务端；
//! - UDP 收到密文 → `Tunn::decapsulate` → 写回 TUN（或回送握手包）；
//! - 定时 `Tunn::update_timers` → 维护握手 / persistent-keepalive。
//!
//! 运行要求：仅需 root/管理员（开 TUN 设备），**无需安装任何 WireGuard 工具**。
//! Windows 额外需随包分发的 `wintun.dll`（由 `tun` 依赖加载，非用户安装）。

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use base64::Engine;
use boringtun::noise::{Tunn, TunnResult};
use boringtun::x25519::{PublicKey, StaticSecret};
use net_route::{Handle, Route};
use tokio::net::UdpSocket;
use tokio::sync::watch;
use tracing::Instrument;
use tun::AbstractDevice;
use vpn_api_types::peer::{ClientDnsSettings, ObfsMode};
use vpn_api_types::system::{obfs_transport_safe_mtu, NetworkMtuMode, NetworkSettings};
use vpn_obfs::{Codec, Direction, Mode, ReplayCache};
use zeroize::Zeroizing;

use crate::daemon::{RoutePolicy, SharedState, TunnelTransport};
use crate::error::{CliError, CliResult};

/// 恢复只依赖认证后的 WireGuard 报文，不依赖业务网站或控制面心跳。
#[derive(Default)]
struct TunnelRecovery {
    last_authenticated_rx: Option<Duration>,
    last_attempt: Option<Duration>,
    attempts: u32,
}
impl TunnelRecovery {
    fn authenticated_received(&mut self, now: Duration) {
        // 使用转发循环的同一个时钟。boringtun 的握手时间来自上一轮定时器，
        // 不能用它推算握手是否发生在本次恢复之后。
        self.last_authenticated_rx = Some(now);
        self.last_attempt = None;
        self.attempts = 0;
    }

    fn tick(&mut self, now: Duration, expired: bool) -> (bool, bool) {
        if !expired
            && self.last_attempt.is_none()
            && self
                .last_authenticated_rx
                .is_some_and(|last| now.saturating_sub(last) < Duration::from_secs(180))
        {
            return (true, false);
        }
        let delay = Duration::from_secs((15u64 << self.attempts.min(2)).min(60));
        let due = match self.last_attempt {
            Some(last) => now.saturating_sub(last) >= delay,
            None => expired || now >= Duration::from_secs(15),
        };
        if due {
            self.last_attempt = Some(now);
            self.attempts = self.attempts.saturating_add(1);
        }
        (false, due)
    }
}

async fn renewed_udp(old: &UdpSocket) -> std::io::Result<UdpSocket> {
    let peer = old.peer_addr()?;
    let socket = UdpSocket::bind(if peer.is_ipv6() {
        "[::]:0"
    } else {
        "0.0.0.0:0"
    })
    .await?;
    socket.connect(peer).await?;
    Ok(socket)
}

const IP_UDP_OVERHEAD: u16 = 28;
const WG_OVERHEAD: u16 = 32;
const WG_BLOCK_SIZE: usize = 16;
/// 定时器步进：boringtun 建议 ~100–250ms 调一次 update_timers。
const TIMER_TICK: Duration = Duration::from_millis(250);
const SEND_ERROR_LOG_INTERVAL: Duration = Duration::from_secs(10);
// boringtun 的内部队列上限为 256；启动时空报文会占一个位置用于 keepalive。
const MAX_TRACKED_QUEUED_PACKETS: usize = 255;

#[derive(Debug)]
struct NetworkSendError {
    stage: &'static str,
    message: String,
}

#[derive(Debug, Default)]
struct SendFailureLogger {
    entries: HashMap<(&'static str, &'static str), (Option<std::time::Instant>, u64)>,
}

#[derive(Debug, Default)]
struct SendState {
    failure_logger: SendFailureLogger,
    pending_tx_lengths: VecDeque<u64>,
}

impl SendState {
    fn track_pending_tx(&mut self, original_len: usize) {
        if original_len > 0 && self.pending_tx_lengths.len() < MAX_TRACKED_QUEUED_PACKETS {
            self.pending_tx_lengths.push_back(original_len as u64);
        }
    }

    fn clear_pending_tx(&mut self) -> usize {
        let cleared = self.pending_tx_lengths.len();
        self.pending_tx_lengths.clear();
        cleared
    }
}

impl SendFailureLogger {
    fn record(&mut self, operation: &'static str, packet: &[u8], error: &NetworkSendError) {
        let now = std::time::Instant::now();
        let entry = self
            .entries
            .entry((error.stage, operation))
            .or_insert((None, 0));
        let should_log = entry
            .0
            .is_none_or(|last| now.duration_since(last) >= SEND_ERROR_LOG_INTERVAL);
        if should_log {
            tracing::warn!(
                stage = error.stage,
                result = "failed",
                operation,
                wireguard_type = wireguard_packet_type(packet).unwrap_or_default(),
                packet_len = packet.len(),
                suppressed = entry.1,
                error = %crate::error::redact_sensitive(&error.message),
                "WireGuard 数据面操作失败"
            );
            *entry = (Some(now), 0);
        } else {
            entry.1 = entry.1.saturating_add(1);
        }
    }
}

fn wireguard_packet_type(packet: &[u8]) -> Option<u32> {
    let header: [u8; 4] = packet.get(..4)?.try_into().ok()?;
    let packet_type = u32::from_le_bytes(header);
    (1..=4).contains(&packet_type).then_some(packet_type)
}

/// 在 WireGuard 加密前将非空 IP 报文零填充至 16 字节边界。
///
/// 返回包含填充的切片；`original_len` 仍应由调用方用于用户流量统计。
fn pad_wireguard_plaintext(buffer: &mut [u8], original_len: usize) -> CliResult<&[u8]> {
    if original_len > buffer.len() {
        return Err(CliError::Other(format!(
            "WireGuard 明文长度 {original_len} 超过缓冲区容量 {}",
            buffer.len()
        )));
    }
    if original_len == 0 {
        return Ok(&buffer[..0]);
    }
    let padding = (WG_BLOCK_SIZE - original_len % WG_BLOCK_SIZE) % WG_BLOCK_SIZE;
    let padded_len = original_len
        .checked_add(padding)
        .ok_or_else(|| CliError::Other("WireGuard 明文填充长度溢出".to_string()))?;
    if padded_len > buffer.len() {
        return Err(CliError::Other(format!(
            "WireGuard 明文填充后长度 {padded_len} 超过缓冲区容量 {}",
            buffer.len()
        )));
    }
    buffer[original_len..padded_len].fill(0);
    Ok(&buffer[..padded_len])
}

/// 解析 base64 WireGuard 密钥为 32 字节。
fn decode_key(b64: &str) -> CliResult<[u8; 32]> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .map_err(|e| CliError::Other(format!("密钥 base64 解码失败: {e}")))?;
    bytes
        .as_slice()
        .try_into()
        .map_err(|_| CliError::Other("WireGuard 密钥长度非 32 字节".to_string()))
}

#[derive(Debug)]
struct ObfsRuntime {
    encoder: Codec,
    decoder: Codec,
    replay: ReplayCache,
}

fn build_obfs_runtime(transport: &TunnelTransport) -> CliResult<ObfsRuntime> {
    let psk = Zeroizing::new(
        base64::engine::general_purpose::STANDARD
            .decode(transport.psk.trim())
            .map_err(|_| CliError::Invalid("混淆 PSK Base64 非法".to_string()))?,
    );
    if psk.len() != 32 || !(576..=9000).contains(&transport.path_mtu) {
        return Err(CliError::Invalid(
            "混淆 PSK 或 path MTU 配置非法".to_string(),
        ));
    }
    let mode = match transport.mode {
        ObfsMode::LowOverheadV1 => Mode::LowOverheadV1,
        ObfsMode::ParanoidV1 => Mode::ParanoidV1,
    };
    let max_datagram = usize::from(transport.path_mtu - IP_UDP_OVERHEAD);
    let encoder = Codec::new(&psk, mode, Direction::ClientToServer, max_datagram)
        .map_err(|error| CliError::Invalid(error.to_string()))?;
    let decoder = Codec::new(&psk, mode, Direction::ServerToClient, max_datagram)
        .map_err(|error| CliError::Invalid(error.to_string()))?;
    Ok(ObfsRuntime {
        encoder,
        decoder,
        replay: ReplayCache::new(),
    })
}

fn transport_safe_mtu(transport: &TunnelTransport) -> u16 {
    obfs_transport_safe_mtu(transport.mode, transport.path_mtu)
}

fn tunnel_mtu(transport: Option<&TunnelTransport>, settings: &NetworkSettings) -> CliResult<u16> {
    settings
        .validate()
        .map_err(|error| CliError::Invalid(format!("网络参数非法：{error}")))?;
    let mtu = match settings.mode {
        NetworkMtuMode::Fixed => settings.default_mtu,
        NetworkMtuMode::Auto => transport
            .map(transport_safe_mtu)
            .unwrap_or(settings.default_mtu)
            .clamp(settings.min_mtu, settings.max_mtu),
    };
    if let Some(transport) = transport {
        let safe_mtu = transport_safe_mtu(transport);
        if mtu > safe_mtu {
            return Err(CliError::Invalid(format!(
                "网络参数 MTU {mtu} 超过当前混淆路径可承载的 {safe_mtu}"
            )));
        }
    }
    Ok(mtu)
}

/// 前缀长度 → IPv4 子网掩码（如 24 → 255.255.255.0）。
fn prefix_to_netmask_v4(prefix: u8) -> Ipv4Addr {
    if prefix == 0 {
        Ipv4Addr::UNSPECIFIED
    } else {
        Ipv4Addr::from(u32::MAX << (32 - prefix.min(32) as u32))
    }
}

/// 解析 `a.b.c.d/n` 为 (网络地址, 前缀)。
#[cfg(test)]
fn parse_cidr_v4(s: &str) -> Option<(Ipv4Addr, u8)> {
    let (ip, pfx) = s.trim().split_once('/')?;
    let ip: Ipv4Addr = ip.parse().ok()?;
    let pfx: u8 = pfx.parse().ok()?;
    if pfx > 32 {
        return None;
    }
    Some((ip, pfx))
}

/// Keep configured destinations on the TUN. The TCP router chooses a path per
/// connection, so it must never install a physical /32 that redirects other
/// connections to the same address. Retain the VPN subnet as its own route.
fn tunnel_routes(allowed: &[String], vpn: ipnet::Ipv4Net, ifindex: u32) -> Vec<Route> {
    let mut networks = BTreeMap::new();
    for cidr in allowed {
        let Ok(network) = cidr.trim().parse::<ipnet::Ipv4Net>() else {
            continue;
        };
        if network.prefix_len() == 0 {
            continue;
        }
        let network = network.trunc();
        networks.insert(
            network,
            Route::new(network.network().into(), network.prefix_len()).with_ifindex(ifindex),
        );
    }
    if vpn.prefix_len() != 0 {
        let vpn = vpn.trunc();
        networks.insert(
            vpn,
            Route::new(vpn.network().into(), vpn.prefix_len()).with_ifindex(ifindex),
        );
    }
    networks.into_values().collect()
}

/// Rules only remove this client's VPN routes. The operating system retains
/// control of all physical/default/third-party routes for excluded destinations.
fn routes_for_local_addresses(
    policy: &RoutePolicy,
    vpn: ipnet::Ipv4Net,
    ifindex: u32,
    addresses: &[Ipv4Addr],
) -> Result<(Vec<Route>, Vec<ipnet::Ipv4Net>), String> {
    let rules = vpn_api_types::system::normalize_local_route_bypass(&policy.local_route_bypass)?;
    let mut exclusions = std::collections::BTreeSet::new();
    for rule in rules {
        let matches = rule.local_subnets.iter().any(|cidr| {
            cidr.parse::<ipnet::Ipv4Net>()
                .is_ok_and(|network| addresses.iter().any(|address| network.contains(address)))
        });
        if matches {
            for cidr in rule.excluded_routes {
                exclusions.insert(cidr.parse::<ipnet::Ipv4Net>().map_err(|e| e.to_string())?);
            }
        }
    }
    let exclusions: Vec<_> = exclusions.into_iter().collect();
    if exclusions.is_empty() {
        return Ok((
            tunnel_routes(&policy.allowed_routes, vpn, ifindex),
            exclusions,
        ));
    }
    let networks = crate::route_bypass::effective_routes(&policy.allowed_routes, vpn, &exclusions)
        .map_err(|error| error.to_string())?;
    Ok((
        networks
            .into_iter()
            .map(|network| {
                Route::new(network.network().into(), network.prefix_len()).with_ifindex(ifindex)
            })
            .collect(),
        exclusions,
    ))
}

fn current_tunnel_routes(
    policy: &RoutePolicy,
    vpn: ipnet::Ipv4Net,
    ifindex: u32,
) -> (Vec<Route>, Vec<ipnet::Ipv4Net>) {
    if policy.local_route_bypass.is_empty() {
        return (tunnel_routes(&policy.allowed_routes, vpn, ifindex), vec![]);
    }
    let plan = crate::local_network::physical_ipv4_addresses(ifindex)
        .map_err(|error| error.to_string())
        .and_then(|addresses| routes_for_local_addresses(policy, vpn, ifindex, &addresses));
    match plan {
        Ok(plan) => plan,
        Err(error) => {
            // A failed network read or oversized plan must restore the complete
            // VPN route set, never retain exclusions from a previous network.
            tracing::warn!(stage = "local_route_bypass", %error, "无法应用条件路由排除，本次保留完整 VPN 路由");
            (tunnel_routes(&policy.allowed_routes, vpn, ifindex), vec![])
        }
    }
}

fn record_route_exclusions(active: &mut Vec<ipnet::Ipv4Net>, next: Vec<ipnet::Ipv4Net>) {
    if *active != next {
        tracing::info!(
            stage = "local_route_bypass",
            excluded_routes = ?next,
            "匹配的路由排除策略已更新，路由操作失败时将继续重试"
        );
        *active = next;
    }
}

/// 用户态隧道句柄（保留拆除所需信息）。任务在 shutdown 信号后自行清理路由并退出。
pub struct UserspaceTunnel;

impl UserspaceTunnel {
    /// 建立隧道并启动后台转发任务（零外部命令）。
    ///
    /// 同步阶段（可失败→返回 Err 供上层报错）：开 TUN + 配 IP + 加路由 + 连 UDP；
    /// 之后 `tokio::spawn` 长期转发循环，循环在 `shutdown` 置位后删除自己加的路由并退出。
    #[allow(clippy::too_many_arguments)]
    pub async fn bring_up(
        iface: &str,
        client_private_key: &str,
        server_public_key: &str,
        server_endpoint: &str,
        transport: Option<&TunnelTransport>,
        network_settings: &NetworkSettings,
        dns_settings: Option<&ClientDnsSettings>,
        vpn_ip: Ipv4Addr,
        subnet_prefix: u8,
        allowed_routes: &[String],
        local_route_bypass: &[vpn_api_types::system::LocalRouteBypassRule],
        keepalive_secs: u16,
        shutdown: watch::Receiver<bool>,
        // 转发循环遇致命错误(如 TUN 读失败)时广播关停,连带停掉心跳任务,避免它继续
        // 上报 Connected 掩盖数据面已死。通常传 shutdown 对应的 Sender 的 clone。
        shutdown_tx: watch::Sender<bool>,
        // 流量计数回写目标（前端读 bytes_rx/bytes_tx）；None 时不统计。
        traffic: Option<SharedState>,
        // 实时路由更新：心跳推送允许网段与排除规则，转发循环据此增量
        // 增删本地路由；None 时不支持热更新。
        routes_rx: Option<watch::Receiver<RoutePolicy>>,
    ) -> CliResult<tokio::task::JoinHandle<CliResult<()>>> {
        let bring_up_started = std::time::Instant::now();
        tracing::info!(
            stage = "wireguard_engine",
            result = "started",
            "初始化用户态 WireGuard 引擎"
        );
        let obfs = transport.map(build_obfs_runtime).transpose()?;
        let mtu = tunnel_mtu(transport, network_settings)?;
        tracing::info!(stage = "obfs_client", result = "configured", transport_mode = ?transport.map(|value| value.mode), mtu_mode = ?network_settings.mode, default_mtu = network_settings.default_mtu, min_mtu = network_settings.min_mtu, max_mtu = network_settings.max_mtu, mtu, "客户端数据面传输已配置");
        // 1) boringtun 状态机：本地私钥 + 服务端公钥。
        let static_private = StaticSecret::from(decode_key(client_private_key).inspect_err(|error| {
            tracing::warn!(stage = "wireguard_engine", result = "failed", elapsed_ms = bring_up_started.elapsed().as_millis(), error = %error.safe_diagnostic(), "初始化客户端 WireGuard 密钥失败");
        })?);
        let peer_public = PublicKey::from(decode_key(server_public_key).inspect_err(|error| {
            tracing::warn!(stage = "wireguard_engine", result = "failed", elapsed_ms = bring_up_started.elapsed().as_millis(), error = %error.safe_diagnostic(), "初始化服务端 WireGuard 公钥失败");
        })?);
        let tunn = Tunn::new(
            static_private,
            peer_public,
            None,
            Some(keepalive_secs),
            0,
            None,
        );

        // 2) 解析服务端 endpoint（可能得到多个地址 / IPv4+IPv6,稍后逐个尝试连接）。
        let endpoint_started = std::time::Instant::now();
        tracing::info!(
            stage = "endpoint_resolution",
            result = "started",
            "解析 VPN endpoint"
        );
        let server_addrs: Vec<SocketAddr> = match tokio::net::lookup_host(server_endpoint).await {
            Ok(addresses) => addresses
                .filter(|address| transport.is_none() || address.is_ipv4())
                .collect(),
            Err(error) => {
                tracing::warn!(stage = "endpoint_resolution", result = "failed", elapsed_ms = endpoint_started.elapsed().as_millis(), error = %crate::error::redact_sensitive(&error.to_string()), "解析 VPN endpoint 失败");
                return Err(CliError::Other(format!(
                    "解析服务端 endpoint 失败: {error}"
                )));
            }
        };
        if server_addrs.is_empty() {
            tracing::warn!(
                stage = "endpoint_resolution",
                result = "failed",
                elapsed_ms = endpoint_started.elapsed().as_millis(),
                reason = "no_addresses",
                "VPN endpoint 未解析到地址"
            );
            return Err(CliError::Other(format!(
                "无法解析 endpoint: {server_endpoint}"
            )));
        }
        tracing::info!(
            stage = "endpoint_resolution",
            result = "succeeded",
            elapsed_ms = endpoint_started.elapsed().as_millis(),
            addresses = server_addrs.len(),
            "VPN endpoint 解析完成"
        );

        // 3) 打开并配置 TUN 设备（地址用子网掩码 → 自动连通 VPN 子网）。
        let tun_started = std::time::Instant::now();
        tracing::info!(
            stage = "tun_open",
            result = "started",
            iface,
            "开始创建 TUN 设备"
        );
        let mut cfg = tun::Configuration::default();
        cfg.address(vpn_ip)
            .netmask(prefix_to_netmask_v4(subnet_prefix))
            .mtu(mtu)
            .up();
        // macOS 的 utun 名称由内核分配（必须形如 utunN），不能用自定义名；
        // Linux/Windows 可指定接口名便于识别。
        #[cfg(not(target_os = "macos"))]
        cfg.tun_name(iface);
        // Windows:上层(桌面端)经 VPN_WINTUN_PATH 指定随包分发的 wintun.dll 时,用绝对路径
        // load,避免 tun 默认仅在工作目录查找 wintun.dll;未设置则回退默认搜索。
        #[cfg(target_os = "windows")]
        if let Some(dll) = std::env::var_os("VPN_WINTUN_PATH") {
            cfg.platform_config(|p| {
                p.wintun_file(dll);
            });
        }
        let device = tun::create_as_async(&cfg).map_err(|error| {
            tracing::warn!(stage = "tun_open", result = "failed", elapsed_ms = tun_started.elapsed().as_millis(), error = %crate::error::redact_sensitive(&error.to_string()), "创建 TUN 设备失败");
            CliError::Other(format!("打开 TUN 设备失败（需 root/管理员）: {error}"))
        })?;
        let ifindex = device.tun_index().map_err(|error| {
            tracing::warn!(stage = "tun_open", result = "failed", elapsed_ms = tun_started.elapsed().as_millis(), error = %crate::error::redact_sensitive(&error.to_string()), "读取 TUN 设备索引失败");
            CliError::Other(format!("获取 TUN ifindex 失败: {error}"))
        })? as u32;
        tracing::info!(
            stage = "tun_open",
            result = "succeeded",
            elapsed_ms = tun_started.elapsed().as_millis(),
            ifindex,
            "TUN 设备已就绪"
        );

        // 4) UDP socket，连到服务端。按解析地址的协议族绑定对应 socket(IPv4→0.0.0.0、
        // IPv6→[::]),逐个尝试直到 connect 成功——避免「只绑 IPv4 socket 却拿到 IPv6 地址」
        // 直接失败,以及首个地址不可达时不回退其余地址。
        let udp_started = std::time::Instant::now();
        tracing::info!(
            stage = "udp_connect",
            result = "started",
            candidates = server_addrs.len(),
            "开始连接 VPN UDP endpoint"
        );
        let mut udp_connected: Option<(UdpSocket, SocketAddr)> = None;
        let mut last_err: Option<String> = None;
        for addr in &server_addrs {
            let bind_addr = if addr.is_ipv6() {
                "[::]:0"
            } else {
                "0.0.0.0:0"
            };
            match UdpSocket::bind(bind_addr).await {
                Ok(sock) => match sock.connect(*addr).await {
                    Ok(()) => {
                        udp_connected = Some((sock, *addr));
                        break;
                    }
                    Err(e) => last_err = Some(format!("{addr}: {e}")),
                },
                Err(e) => last_err = Some(format!("bind {bind_addr}: {e}")),
            }
        }
        let (udp, server_addr) = udp_connected.ok_or_else(|| {
            tracing::warn!(stage = "udp_connect", result = "failed", elapsed_ms = udp_started.elapsed().as_millis(), attempts = server_addrs.len(), error = %crate::error::redact_sensitive(last_err.as_deref().unwrap_or_default()), "连接 VPN UDP endpoint 失败");
            CliError::Other(format!(
                "连接服务端 UDP 失败（已尝试全部解析地址）: {}",
                last_err.unwrap_or_default()
            ))
        })?;
        tracing::info!(
            stage = "udp_connect",
            result = "succeeded",
            elapsed_ms = udp_started.elapsed().as_millis(),
            address_family = if server_addr.is_ipv6() {
                "ipv6"
            } else {
                "ipv4"
            },
            "VPN UDP endpoint 已就绪"
        );

        // 5) Configured destinations stay on the TUN. A TCP connection can use
        // a physical socket without changing the routes of other connections.
        let vpn_subnet = ipnet::Ipv4Net::new(vpn_ip, subnet_prefix)
            .map_err(|error| CliError::Invalid(format!("无效 VPN 子网: {error}")))?
            .trunc();
        let handle =
            Handle::new().map_err(|error| CliError::Other(format!("路由句柄失败: {error}")))?;
        let route_policy = RoutePolicy {
            allowed_routes: allowed_routes.to_vec(),
            local_route_bypass: local_route_bypass.to_vec(),
        };
        let (desired, _) = current_tunnel_routes(&route_policy, vpn_subnet, ifindex);
        let mut added = Vec::new();
        crate::route_reconcile::reconcile(&handle, &mut added, &desired).await;
        let route_result = if added.len() == desired.len() {
            "succeeded"
        } else {
            "degraded"
        };
        tracing::info!(
            stage = "route_apply",
            result = route_result,
            requested = desired.len(),
            applied = added.len(),
            "VPN 路由应用完成"
        );

        // 此处只清理旧配置；新 DNS 由转发循环中的健康检测任务延迟应用。
        if let Err(error) = vpn_platform::cleanup_stale_dns(ifindex).await {
            if crate::route_reconcile::cleanup(&handle, &mut added).await > 0 {
                return Err(CliError::Cleanup("DNS 清理失败且路由回滚未完成".into()));
            }
            return Err(CliError::Other(format!("清理遗留客户端 DNS 失败：{error}")));
        }

        tracing::info!(
            stage = "data_plane_ready",
            result = if route_result == "succeeded" { "succeeded" } else { "degraded" },
            elapsed_ms = bring_up_started.elapsed().as_millis(),
            iface,
            %vpn_ip,
            routes = added.len(),
            "用户态 WireGuard 数据面已就绪，启动转发循环（尚未确认握手）"
        );

        // 6) 后台转发任务。返回其 JoinHandle,供上层在重连时等待旧任务清完路由再建新隧道。
        let task = tokio::spawn(
            forward_loop(
                device,
                udp,
                tunn,
                handle,
                added,
                ifindex,
                shutdown,
                shutdown_tx,
                traffic,
                routes_rx,
                obfs,
                dns_settings.cloned(),
                vpn_ip,
                route_policy,
                vpn_subnet,
                usize::from(mtu) + usize::from(WG_OVERHEAD) + 64,
            )
            .instrument(tracing::Span::current()),
        );
        Ok(task)
    }
}

/// 单任务转发循环：TUN ↔ boringtun ↔ UDP，shutdown 后清理路由退出。
#[allow(clippy::too_many_arguments)]
async fn forward_loop(
    device: tun::AsyncDevice,
    mut udp: UdpSocket,
    mut tunn: Tunn,
    handle: Handle,
    mut added_routes: Vec<(String, Route)>,
    ifindex: u32,
    mut shutdown: watch::Receiver<bool>,
    shutdown_tx: watch::Sender<bool>,
    traffic: Option<SharedState>,
    mut routes_rx: Option<watch::Receiver<RoutePolicy>>,
    mut obfs: Option<ObfsRuntime>,
    dns_settings: Option<ClientDnsSettings>,
    vpn_ip: Ipv4Addr,
    mut route_policy: RoutePolicy,
    vpn_subnet: ipnet::Ipv4Net,
    packet_buffer_size: usize,
) -> CliResult<()> {
    // 独立运行探测，避免等待 DNS 时阻塞 TUN 转发。sender 随本任务销毁，
    // 即使 forward_loop panic，monitor 也能收到关闭并恢复 DNS。
    let (dns_stop, dns_rx) = watch::channel(false);
    let dns_task = dns_settings
        .filter(|settings| settings.mode != vpn_api_types::system::ClientDnsMode::Disabled)
        .map(|settings| {
            let stop_connection = shutdown_tx.clone();
            tokio::spawn(async move {
                let result = vpn_platform::monitor_dns(ifindex, vpn_ip, settings, dns_rx).await;
                if result.is_err() {
                    let _ = stop_connection.send(true);
                }
                result
            })
        });
    let loop_started = std::time::Instant::now();
    tracing::info!(
        stage = "forward_loop",
        result = "started",
        "WireGuard 转发循环已启动"
    );
    // 三个方向各用独立缓冲，避免 select! 多分支对同一缓冲的可变借用冲突。
    let mut tun_read_buf = vec![0u8; packet_buffer_size];
    let mut enc_buf = vec![0u8; packet_buffer_size];
    let mut udp_read_buf = vec![0u8; 65535];
    let mut ticker = tokio::time::interval(TIMER_TICK);
    let mut active_exclusions = Vec::new();
    let mut route_retry = tokio::time::interval(Duration::from_secs(5));
    route_retry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // 本地累加收发字节，按 TIMER_TICK 节奏批量刷回 SharedState——避免每包都锁 mutex
    // （高吞吐时每秒数千包，逐包加锁会成为热点）。统计的是隧道明文负载（用户可见的
    // 实际转发量），而非含 WireGuard 封装开销的 UDP 字节。
    let mut tx_acc: u64 = 0; // 出站：写入隧道的明文（Sent）
    let mut rx_acc: u64 = 0; // 入站：从隧道收到的明文（Received）
    let mut udp_send_failures: u64 = 0;
    let mut udp_recv_failures: u64 = 0;
    let mut timer_failures: u64 = 0;
    let mut last_udp_error_log = std::time::Instant::now();
    let mut obfs_drops: u64 = 0;
    let mut send_state = SendState::default();
    let mut recovery = TunnelRecovery::default();

    // 立即发起握手（无 src 触发 handshake initiation）。
    if let TunnResult::WriteToNetwork(p) = tunn.encapsulate(&[], &mut enc_buf) {
        if let Err(error) = send_network(&udp, obfs.as_ref(), p).await {
            send_state
                .failure_logger
                .record("initial_handshake", p, &error);
            udp_send_failures = udp_send_failures.saturating_add(1);
        }
    }

    let outcome = loop {
        tokio::select! {
            res = shutdown.changed() => {
                // 显式置位 true，或 sender 被 drop（通道关闭）→ 退出并在循环末尾清理路由。
                if res.is_err() || *shutdown.borrow() { break Ok(()); }
            }
            // 出站：进入 TUN 的所有 IP 报文直接交给 WireGuard。
            r = device.recv(&mut tun_read_buf) => {
                match r {
                    Ok(n) => {
                        if n == 0 {
                            continue;
                        }
                        match encapsulate_outgoing(
                            &mut tunn,
                            &udp,
                            obfs.as_ref(),
                            &mut tun_read_buf,
                            n,
                            &mut enc_buf,
                            &mut send_state,
                        ).await {
                            Ok((tx_bytes, failures)) => {
                                tx_acc = tx_acc.saturating_add(tx_bytes);
                                udp_send_failures = udp_send_failures.saturating_add(failures);
                            }
                            Err(error) => {
                                stop_data_plane(&traffic, &shutdown_tx, &error).await;
                                break Err(error);
                            }
                        }
                    }
                    Err(e) => {
                        // TUN 读失败是数据面致命错误:置错误态并广播关停(连带停掉心跳),
                        // 避免转发循环静默退出后心跳仍上报 Connected、UI 显示"已连接"实则零流量。
                        tracing::warn!(error = %e, "TUN 读失败，数据面停止");
                        if let Some(s) = &traffic {
                            s.set_error(format!("数据面中断(TUN 读失败): {e}"), crate::daemon::now_unix())
                                .await;
                        }
                        let _ = shutdown_tx.send(true);
                        break Err(CliError::Other(format!("TUN 读取失败: {e}")));
                    }
                }
            }
            // 入站：UDP → 解密 → TUN（或回送握手）
            r = udp.recv(&mut udp_read_buf) => {
                match r {
                    Ok(n) => {
                        let decoded;
                        let incoming = if let Some(runtime) = obfs.as_mut() {
                            match runtime.decoder.decode(&udp_read_buf[..n], &mut runtime.replay) {
                                Ok(packet) => { decoded = packet; decoded.as_slice() }
                                Err(_) => {
                                    obfs_drops = obfs_drops.saturating_add(1);
                                    continue;
                                }
                            }
                        } else {
                            &udp_read_buf[..n]
                        };
                        match handle_incoming(
                            &mut tunn,
                            &udp,
                            &device,
                            incoming,
                            obfs.as_ref(),
                            packet_buffer_size,
                            &mut send_state,
                        ).await {
                            Ok(IncomingStats { rx_bytes, tx_bytes, send_failures, authenticated }) => {
                                if authenticated {
                                    recovery.authenticated_received(loop_started.elapsed());
                                }
                                rx_acc = rx_acc.saturating_add(rx_bytes);
                                tx_acc = tx_acc.saturating_add(tx_bytes);
                                udp_send_failures = udp_send_failures.saturating_add(send_failures);
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "写入 TUN 失败，数据面停止");
                                if let Some(s) = &traffic {
                                    s.set_error(format!("数据面中断: {e}"), crate::daemon::now_unix()).await;
                                }
                                let _ = shutdown_tx.send(true);
                                break Err(e);
                            }
                        }
                    }
                    // UDP 瞬时错误**不拆隧道**：connected UDP socket 在对端暂不可达时会收到
                    // ICMP port-unreachable → recv 返回 ConnectionRefused;网络切换/抖动同理。
                    // 忽略续跑，boringtun 的定时器(下方 ticker)会自动重握手恢复。短暂 sleep
                    // 避免错误持续返回时空转占 CPU。
                    Err(_) => {
                        udp_recv_failures = udp_recv_failures.saturating_add(1);
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                }
            }
            // 实时路由更新：心跳检测到允许网段或排除规则变化 → 增量增删本地路由。
            changed = wait_routes_change(&mut routes_rx) => {
                if changed {
                    if let Some(rx) = routes_rx.as_ref() {
                        route_policy = rx.borrow().clone();
                        let (desired, exclusions) = current_tunnel_routes(&route_policy, vpn_subnet, ifindex);
                        crate::route_reconcile::reconcile(&handle, &mut added_routes, &desired).await;
                        record_route_exclusions(&mut active_exclusions, exclusions);
                    }
                }
            }
            _ = route_retry.tick() => {
                // Retry failed OS operations even if configuration is unchanged.
                let (desired, exclusions) = current_tunnel_routes(&route_policy, vpn_subnet, ifindex);
                crate::route_reconcile::reconcile(&handle, &mut added_routes, &desired).await;
                record_route_exclusions(&mut active_exclusions, exclusions);
            }
            // 定时器：握手重传 / keepalive，并顺带把累计流量刷回状态。
            _ = ticker.tick() => {
                let mut tbuf = vec![0u8; packet_buffer_size];
                let mut expired = false;
                match tunn.update_timers(&mut tbuf) {
                    TunnResult::WriteToNetwork(p) => {
                        if let Err(error) = send_network(&udp, obfs.as_ref(), p).await {
                            send_state.failure_logger.record("timer", p, &error);
                            udp_send_failures = udp_send_failures.saturating_add(1);
                        }
                    }
                    TunnResult::Err(error) => {
                        expired = matches!(error, boringtun::noise::errors::WireGuardError::ConnectionExpired);
                        let cleared_pending = send_state.clear_pending_tx();
                        let diagnostic = NetworkSendError {
                            stage: "wireguard_timer",
                            message: format!("{error:?}; cleared_pending={cleared_pending}"),
                        };
                        send_state
                            .failure_logger
                            .record("timer_state", &[], &diagnostic);
                        timer_failures = timer_failures.saturating_add(1);
                    }
                    _ => {}
                }
                let (healthy, retry) = recovery.tick(loop_started.elapsed(), expired);
                if let Some(state) = &traffic { state.set_channel_health(true, healthy).await; }
                if retry && !*shutdown.borrow() {
                    // 新 socket 更新源端口/NAT 映射；保留 TUN、路由与密钥，不与退出清理竞态。
                    match renewed_udp(&udp).await {
                        Ok(socket) => udp = socket,
                        Err(error) => tracing::warn!(error=%error, "恢复 UDP socket 失败，保留旧 socket 稍后重试"),
                    }
                    match tunn.format_handshake_initiation(&mut tbuf, true) {
                        TunnResult::WriteToNetwork(packet) => {
                            if let Err(error) = send_network(&udp, obfs.as_ref(), packet).await {
                                send_state.failure_logger.record("recovery_handshake", packet, &error);
                            }
                        }
                        TunnResult::Err(error) => tracing::warn!(?error, "重新发起 WireGuard 握手失败"),
                        _ => {}
                    }
                    tracing::warn!(attempt=recovery.attempts, "数据隧道未就绪，已更新 UDP socket 并重新发起握手");
                }
                if (udp_send_failures > 0
                    || udp_recv_failures > 0
                    || timer_failures > 0
                    || obfs_drops > 0)
                    && last_udp_error_log.elapsed() >= Duration::from_secs(10)
                {
                    tracing::debug!(send_failures = udp_send_failures, recv_failures = udp_recv_failures, timer_failures, obfs_drops, "隧道 UDP 暂时不可用或收到非法混淆包，保持连接等待恢复");
                    udp_send_failures = 0;
                    udp_recv_failures = 0;
                    timer_failures = 0;
                    obfs_drops = 0;
                    last_udp_error_log = std::time::Instant::now();
                }
                if (tx_acc | rx_acc) != 0 {
                    if let Some(t) = &traffic {
                        t.add_traffic(rx_acc, tx_acc).await;
                    }
                    tx_acc = 0;
                    rx_acc = 0;
                }
            }
        }
    };

    // 退出前最后一次刷新（落袋未统计的尾包）。
    if (tx_acc | rx_acc) != 0 {
        if let Some(t) = &traffic {
            t.add_traffic(rx_acc, tx_acc).await;
        }
    }

    let mut cleanup_failures = 0;
    // 清理：先恢复 DNS，再删除本任务加的路由（TUN 设备随 device drop 关闭）。
    let _ = dns_stop.send(true);
    if let Some(task) = dns_task {
        match task.await {
            Ok(Ok(())) => {}
            result => {
                cleanup_failures += 1;
                tracing::warn!(stage = "dns_restore", ?result, "DNS 维护任务失败");
            }
        }
    }
    cleanup_failures += crate::route_reconcile::cleanup(&handle, &mut added_routes).await;
    if cleanup_failures == 0 {
        tracing::info!(
            stage = "forward_loop",
            result = "stopped",
            elapsed_ms = loop_started.elapsed().as_millis(),
            "用户态 WireGuard 转发循环已退出，路由已清理"
        );
    } else {
        tracing::warn!(
            stage = "route_cleanup",
            result = "partial_failure",
            elapsed_ms = loop_started.elapsed().as_millis(),
            cleanup_failures,
            "用户态 WireGuard 已退出，但部分路由清理失败"
        );
        // 清理结果比运行期错误更关键：上层需要据此决定是否
        // fail-closed 阻止重连。即使运行期也出过错，仍要返回专用清理错误。
        return Err(CliError::Cleanup(format!(
            "VPN 路由或 DNS 清理失败: {cleanup_failures} 项"
        )));
    }
    outcome
}

/// 等待路由更新通道有新值;通道为 None(不支持热更新)时永不就绪——该 select 分支不触发。
async fn wait_routes_change(rx: &mut Option<watch::Receiver<RoutePolicy>>) -> bool {
    match rx {
        Some(r) => {
            if r.changed().await.is_ok() {
                true
            } else {
                // sender 已 drop（心跳任务退出）：不再有路由更新。永久 pending，避免对已关闭
                // 通道反复立即就绪而空转打满 CPU；隧道退出改由 shutdown 分支负责。
                std::future::pending::<()>().await;
                false
            }
        }
        None => {
            std::future::pending::<()>().await;
            false
        }
    }
}

/// Encapsulate TUN traffic. Count plaintext bytes only when WireGuard data is sent;
/// keep lengths for packets queued behind a handshake in the existing ledger.
async fn encapsulate_outgoing(
    tunn: &mut Tunn,
    udp: &UdpSocket,
    obfs: Option<&ObfsRuntime>,
    buffer: &mut [u8],
    original_len: usize,
    encrypted: &mut [u8],
    send_state: &mut SendState,
) -> CliResult<(u64, u64)> {
    let capacity = buffer.len();
    let plaintext = pad_wireguard_plaintext(buffer, original_len).inspect_err(|error| {
        tracing::warn!(
            stage = "wireguard_padding",
            result = "failed",
            original_len,
            buffer_capacity = capacity,
            error = %error.safe_diagnostic(),
            "WireGuard 业务报文填充失败，数据面停止"
        );
    })?;
    let padded_len = plaintext.len();
    match tunn.encapsulate(plaintext, encrypted) {
        TunnResult::WriteToNetwork(packet) => {
            let is_data = wireguard_packet_type(packet) == Some(4);
            let failed = match send_network(udp, obfs, packet).await {
                Ok(()) => false,
                Err(error) => {
                    send_state.failure_logger.record("tun_data", packet, &error);
                    true
                }
            };
            if !is_data {
                send_state.track_pending_tx(original_len);
            }
            Ok((
                if is_data && !failed {
                    original_len as u64
                } else {
                    0
                },
                u64::from(failed),
            ))
        }
        TunnResult::Done => {
            send_state.track_pending_tx(original_len);
            Ok((0, 0))
        }
        TunnResult::Err(error) => {
            tracing::warn!(
                stage = "wireguard_encapsulate",
                result = "failed",
                original_len,
                padded_len,
                error = ?error,
                "WireGuard 业务报文封装失败"
            );
            Ok((0, 0))
        }
        _ => Ok((0, 0)),
    }
}

async fn stop_data_plane(
    traffic: &Option<SharedState>,
    shutdown: &watch::Sender<bool>,
    error: &CliError,
) {
    tracing::warn!(error = %error.safe_diagnostic(), "VPN 数据面停止");
    if let Some(state) = traffic {
        state
            .set_error(
                format!("数据面中断: {}", error.safe_diagnostic()),
                crate::daemon::now_unix(),
            )
            .await;
    }
    let _ = shutdown.send(true);
}

#[derive(Default)]
struct IncomingStats {
    rx_bytes: u64,
    tx_bytes: u64,
    send_failures: u64,
    authenticated: bool,
}

/// 只有解密/认证成功的响应和传输包能确认数据通道恢复。
/// Cookie、握手请求、错误和排空队列的空输入均不能作为连接成功的证据。
fn authenticated_receive(packet: &[u8], result: &TunnResult<'_>) -> bool {
    match (wireguard_packet_type(packet), result) {
        (Some(2), TunnResult::WriteToNetwork(reply)) => wireguard_packet_type(reply) == Some(4),
        (
            Some(4),
            TunnResult::Done | TunnResult::WriteToTunnelV4(..) | TunnResult::WriteToTunnelV6(..),
        ) => true,
        _ => false,
    }
}

/// 处理一个入站 UDP 数据报：解密后写回 TUN；握手响应回送网络；并排空队列。
///
/// 返回流量统计和认证结果；握手 / keepalive 不计入明文流量。
#[allow(clippy::too_many_arguments)]
async fn handle_incoming(
    tunn: &mut Tunn,
    udp: &UdpSocket,
    device: &tun::AsyncDevice,
    packet: &[u8],
    obfs: Option<&ObfsRuntime>,
    packet_buffer_size: usize,
    send_state: &mut SendState,
) -> CliResult<IncomingStats> {
    let mut out = vec![0u8; packet_buffer_size];
    let result = tunn.decapsulate(None, packet, &mut out);
    let authenticated = authenticated_receive(packet, &result);
    match result {
        TunnResult::WriteToNetwork(p) => {
            let mut send_failures = 0u64;
            let mut tx_bytes = 0u64;
            if let Err(error) = send_network(udp, obfs, p).await {
                send_state
                    .failure_logger
                    .record("handshake_response", p, &error);
                send_failures = send_failures.saturating_add(1);
            }
            // boringtun 约定：收到握手类响应后，用空包重复调用以排空待发队列。
            loop {
                let mut drain = vec![0u8; packet_buffer_size];
                match tunn.decapsulate(None, &[], &mut drain) {
                    TunnResult::WriteToNetwork(p) => {
                        let queued_user_bytes =
                            if wireguard_packet_type(p) == Some(4) && p.len() > 32 {
                                send_state.pending_tx_lengths.pop_front()
                            } else {
                                None
                            };
                        match send_network(udp, obfs, p).await {
                            Ok(()) => {
                                if let Some(bytes) = queued_user_bytes {
                                    tx_bytes = tx_bytes.saturating_add(bytes);
                                }
                            }
                            Err(error) => {
                                send_state.failure_logger.record("queue_drain", p, &error);
                                send_failures = send_failures.saturating_add(1);
                            }
                        }
                    }
                    TunnResult::WriteToTunnelV4(p, _) | TunnResult::WriteToTunnelV6(p, _) => {
                        device
                            .send(p)
                            .await
                            .map_err(|e| CliError::Other(format!("写入 TUN 失败: {e}")))?;
                    }
                    _ => break,
                }
            }
            Ok(IncomingStats {
                tx_bytes,
                send_failures,
                authenticated,
                ..Default::default()
            })
        }
        TunnResult::WriteToTunnelV4(p, _) | TunnResult::WriteToTunnelV6(p, _) => {
            let n = p.len() as u64;
            device
                .send(p)
                .await
                .map_err(|e| CliError::Other(format!("写入 TUN 失败: {e}")))?;
            Ok(IncomingStats {
                rx_bytes: n,
                authenticated,
                ..Default::default()
            })
        }
        TunnResult::Done => Ok(IncomingStats {
            authenticated,
            ..Default::default()
        }),
        TunnResult::Err(e) => {
            tracing::debug!(?e, "decapsulate 错误（忽略单包）");
            Ok(IncomingStats::default())
        }
    }
}

async fn send_network(
    udp: &UdpSocket,
    obfs: Option<&ObfsRuntime>,
    packet: &[u8],
) -> Result<(), NetworkSendError> {
    if let Some(runtime) = obfs {
        let encoded = runtime
            .encoder
            .encode(packet)
            .map_err(|error| NetworkSendError {
                stage: "obfs_encode",
                message: error.to_string(),
            })?;
        udp.send(&encoded).await.map_err(|error| NetworkSendError {
            stage: "udp_send",
            message: error.to_string(),
        })?;
    } else {
        udp.send(packet).await.map_err(|error| NetworkSendError {
            stage: "udp_send",
            message: error.to_string(),
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bypass_policy() -> RoutePolicy {
        RoutePolicy {
            allowed_routes: vec![
                "10.8.0.0/24".into(),
                "192.168.186.0/23".into(),
                "192.168.188.0/24".into(),
                "172.0.0.0/8".into(),
            ],
            local_route_bypass: vec![vpn_api_types::system::LocalRouteBypassRule {
                local_subnets: vec!["192.168.187.0/24".into()],
                excluded_routes: vec![
                    "192.168.186.0/24".into(),
                    "192.168.187.0/24".into(),
                    "192.168.188.0/24".into(),
                ],
            }],
        }
    }

    fn routes_cover(routes: &[Route], address: &str) -> bool {
        let address: Ipv4Addr = address.parse().unwrap();
        routes.iter().any(|route| {
            let std::net::IpAddr::V4(network) = route.destination else {
                return false;
            };
            ipnet::Ipv4Net::new(network, route.prefix)
                .unwrap()
                .contains(&address)
        })
    }

    #[test]
    fn local_rule_excludes_all_protocol_routes_only_on_matching_network() {
        let policy = bypass_policy();
        let vpn = "10.8.0.0/24".parse().unwrap();
        let (local, exclusions) =
            routes_for_local_addresses(&policy, vpn, 21, &["192.168.187.42".parse().unwrap()])
                .unwrap();
        assert_eq!(exclusions.len(), 3);
        for target in ["192.168.186.1", "192.168.187.2", "192.168.188.1"] {
            assert!(!routes_cover(&local, target), "{target}");
        }
        assert!(routes_cover(&local, "172.30.0.1"));
        assert!(routes_cover(&local, "10.8.0.1"));
        // Moving to another network restores the original VPN route set.
        for addresses in [vec!["192.168.0.103".parse().unwrap()], vec![]] {
            let (restored, exclusions) =
                routes_for_local_addresses(&policy, vpn, 21, &addresses).unwrap();
            assert!(exclusions.is_empty());
            assert_eq!(restored, tunnel_routes(&policy.allowed_routes, vpn, 21));
        }
    }

    #[test]
    fn local_rules_union_matching_interfaces_and_keep_vpn_subnet() {
        let mut policy = bypass_policy();
        policy
            .local_route_bypass
            .push(vpn_api_types::system::LocalRouteBypassRule {
                local_subnets: vec!["10.20.0.0/16".into()],
                excluded_routes: vec!["172.0.0.0/8".into(), "10.0.0.0/8".into()],
            });
        let (routes, _) = routes_for_local_addresses(
            &policy,
            "10.8.0.0/24".parse().unwrap(),
            21,
            &[
                "192.168.187.42".parse().unwrap(),
                "10.20.3.4".parse().unwrap(),
            ],
        )
        .unwrap();
        assert!(!routes_cover(&routes, "172.30.0.1"));
        assert!(!routes_cover(&routes, "192.168.188.1"));
        assert!(routes_cover(&routes, "10.8.0.1"));
        assert_eq!(routes.len(), 1);
    }

    #[test]
    fn local_rule_subtracts_destination_from_broader_allowed_route() {
        let mut policy = bypass_policy();
        policy.allowed_routes = vec!["10.0.0.0/8".into()];
        policy.local_route_bypass[0].excluded_routes = vec!["10.1.0.0/16".into()];
        let (routes, _) = routes_for_local_addresses(
            &policy,
            "10.8.0.0/24".parse().unwrap(),
            21,
            &["192.168.187.42".parse().unwrap()],
        )
        .unwrap();
        assert!(!routes_cover(&routes, "10.1.2.3"));
        assert!(routes_cover(&routes, "10.2.2.3"));
        assert!(routes_cover(&routes, "10.8.0.1"));
    }

    #[test]
    fn invalid_local_rule_never_returns_partial_route_exclusions() {
        let mut policy = bypass_policy();
        policy.local_route_bypass[0]
            .excluded_routes
            .push("not-a-cidr".into());
        assert!(routes_for_local_addresses(
            &policy,
            "10.8.0.0/24".parse().unwrap(),
            21,
            &["192.168.187.42".parse().unwrap(),]
        )
        .is_err());
    }

    #[test]
    fn tunnel_routes_preserve_configured_boundaries_and_vpn_subnet() {
        let vpn = "10.8.0.0/24".parse().unwrap();
        let routes = tunnel_routes(
            &[
                "10.0.0.0/8".into(),
                "192.168.188.111/32".into(),
                "192.168.188.0/24".into(),
            ],
            vpn,
            9,
        );
        assert_eq!(routes.len(), 4);
        for cidr in [
            "10.0.0.0/8",
            "10.8.0.0/24",
            "192.168.188.0/24",
            "192.168.188.111/32",
        ] {
            let net = cidr.parse::<ipnet::Ipv4Net>().unwrap();
            assert!(routes
                .contains(&Route::new(net.network().into(), net.prefix_len()).with_ifindex(9)));
        }
        assert!(routes
            .iter()
            .all(|route| route.ifindex == Some(9) && route.gateway.is_none()));
        assert_eq!(
            tunnel_routes(&[], vpn, 9),
            vec![Route::new("10.8.0.0".parse().unwrap(), 24).with_ifindex(9)]
        );
    }

    #[test]
    fn tunnel_routes_normalize_deduplicate_and_reject_default_ipv6_and_invalid_routes() {
        let routes = tunnel_routes(
            &[
                " 192.168.188.111/24 ".into(),
                "192.168.188.0/24".into(),
                "10.8.0.5/24".into(),
                "0.0.0.0/0".into(),
                "::/0".into(),
                "10.1.1.1/33".into(),
                "invalid".into(),
            ],
            "10.8.0.2/24".parse().unwrap(),
            9,
        );
        assert_eq!(routes.len(), 2);
        assert!(routes.contains(&Route::new("192.168.188.0".parse().unwrap(), 24).with_ifindex(9)));
        assert!(routes.contains(&Route::new("10.8.0.0".parse().unwrap(), 24).with_ifindex(9)));
    }

    #[test]
    fn netmask_from_prefix() {
        assert_eq!(prefix_to_netmask_v4(24), Ipv4Addr::new(255, 255, 255, 0));
        assert_eq!(prefix_to_netmask_v4(16), Ipv4Addr::new(255, 255, 0, 0));
        assert_eq!(prefix_to_netmask_v4(32), Ipv4Addr::new(255, 255, 255, 255));
        assert_eq!(prefix_to_netmask_v4(0), Ipv4Addr::new(0, 0, 0, 0));
    }

    #[test]
    fn parse_cidr_ok_and_bad() {
        assert_eq!(
            parse_cidr_v4("172.31.100.0/24"),
            Some((Ipv4Addr::new(172, 31, 100, 0), 24))
        );
        assert_eq!(
            parse_cidr_v4("10.0.0.0/8"),
            Some((Ipv4Addr::new(10, 0, 0, 0), 8))
        );
        assert!(parse_cidr_v4("nonsense").is_none());
        assert!(parse_cidr_v4("1.2.3.4/33").is_none());
    }

    #[test]
    fn network_policy_controls_and_bounds_tunnel_mtu() {
        let transport = TunnelTransport {
            mode: ObfsMode::ParanoidV1,
            psk: Zeroizing::new(base64::engine::general_purpose::STANDARD.encode([7u8; 32])),
            path_mtu: 1500,
        };
        let fixed = NetworkSettings::default();
        assert_eq!(tunnel_mtu(Some(&transport), &fixed).unwrap(), 1360);
        assert!(build_obfs_runtime(&transport).is_ok());
        let automatic = NetworkSettings {
            mode: NetworkMtuMode::Auto,
            ..fixed
        };
        assert_eq!(tunnel_mtu(Some(&transport), &automatic).unwrap(), 1392);
        let mut minimum = transport.clone();
        minimum.mode = ObfsMode::LowOverheadV1;
        minimum.path_mtu = 576;
        assert!(tunnel_mtu(Some(&minimum), &automatic).is_err());
        let mut jumbo = transport.clone();
        jumbo.path_mtu = 9000;
        assert_eq!(tunnel_mtu(Some(&jumbo), &automatic).unwrap(), 1420);
        assert_eq!(tunnel_mtu(None, &automatic).unwrap(), 1360);
        let unsafe_fixed = NetworkSettings {
            default_mtu: 1420,
            ..NetworkSettings::default()
        };
        assert!(tunnel_mtu(Some(&transport), &unsafe_fixed).is_err());
    }

    #[test]
    fn decode_key_validates_length() {
        // 32 字节 base64（全 0）
        let z = base64::engine::general_purpose::STANDARD.encode([0u8; 32]);
        assert!(decode_key(&z).is_ok());
        assert!(decode_key("not-base64!!").is_err());
        // 合法 base64 但长度不对
        let short = base64::engine::general_purpose::STANDARD.encode([0u8; 16]);
        assert!(decode_key(&short).is_err());
    }

    #[test]
    fn wireguard_plaintext_padding_covers_empty_aligned_and_boundaries() {
        let mut buffer = [0xa5; 1440];

        assert_eq!(pad_wireguard_plaintext(&mut buffer, 0).unwrap().len(), 0);
        assert_eq!(pad_wireguard_plaintext(&mut buffer, 16).unwrap().len(), 16);
        assert_eq!(pad_wireguard_plaintext(&mut buffer, 96).unwrap().len(), 96);
        assert_eq!(
            pad_wireguard_plaintext(&mut buffer, 1424).unwrap().len(),
            1424
        );

        let padded = pad_wireguard_plaintext(&mut buffer, 15).unwrap();
        assert_eq!(padded.len(), 16);
        assert_eq!(padded[15], 0);
        let padded = pad_wireguard_plaintext(&mut buffer, 17).unwrap();
        assert_eq!(padded.len(), 32);
        assert!(padded[17..].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn wireguard_plaintext_padding_preserves_packet_and_zero_fills() {
        let original: Vec<u8> = (0..84).map(|value| value as u8).collect();
        let mut buffer = vec![0xa5; 96];
        buffer[..original.len()].copy_from_slice(&original);

        let padded = pad_wireguard_plaintext(&mut buffer, original.len()).unwrap();

        assert_eq!(padded.len(), 96);
        assert_eq!(&padded[..original.len()], original.as_slice());
        assert!(padded[original.len()..].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn wireguard_plaintext_padding_rejects_insufficient_capacity() {
        let mut exact_original = [0u8; 84];
        assert!(pad_wireguard_plaintext(&mut exact_original, 84).is_err());

        let mut short = [0u8; 16];
        assert!(pad_wireguard_plaintext(&mut short, 17).is_err());
    }

    #[test]
    fn wireguard_packet_type_requires_complete_little_endian_header() {
        assert_eq!(wireguard_packet_type(&[]), None);
        assert_eq!(wireguard_packet_type(&[4]), None);
        assert_eq!(wireguard_packet_type(&[4, 0, 0]), None);
        assert_eq!(wireguard_packet_type(&[1, 0, 0, 0]), Some(1));
        assert_eq!(wireguard_packet_type(&[4, 0, 0, 0, 99]), Some(4));
        assert_eq!(wireguard_packet_type(&[4, 1, 0, 0]), None);
        assert_eq!(wireguard_packet_type(&[0, 0, 0, 0]), None);
        assert_eq!(wireguard_packet_type(&[5, 0, 0, 0]), None);
    }

    #[test]
    fn send_state_ignores_empty_packets_and_clears_pending_on_session_error() {
        let mut state = SendState::default();
        state.track_pending_tx(0);
        assert!(state.pending_tx_lengths.is_empty());

        state.track_pending_tx(84);
        state.track_pending_tx(96);
        assert_eq!(state.clear_pending_tx(), 2);
        assert!(state.pending_tx_lengths.is_empty());
        assert_eq!(state.clear_pending_tx(), 0);
    }

    #[test]
    fn recovery_is_bounded_and_requires_new_authenticated_traffic() {
        let secs = Duration::from_secs;
        let mut recovery = TunnelRecovery::default();
        assert_eq!(recovery.tick(secs(0), false), (false, false));
        assert_eq!(recovery.tick(secs(15), false), (false, true));
        assert_eq!(recovery.tick(secs(16), true), (false, false));
        assert_eq!(recovery.tick(secs(44), true), (false, false));
        assert_eq!(recovery.tick(secs(45), true), (false, true));
        assert_eq!(recovery.tick(secs(104), true), (false, false));
        assert_eq!(recovery.tick(secs(105), true), (false, true));
        recovery.authenticated_received(secs(105));
        assert_eq!(recovery.tick(secs(105), false), (true, false));
        assert_eq!(recovery.attempts, 0);
        // 连接过期后，缓存的认证记录不能结束新一轮恢复。
        assert_eq!(recovery.tick(secs(106), true), (false, true));
        assert_eq!(recovery.tick(secs(107), false), (false, false));
        recovery.authenticated_received(secs(108));
        assert_eq!(recovery.tick(secs(287), false), (true, false));
        assert_eq!(recovery.tick(secs(288), false), (false, true));
    }

    #[test]
    fn only_authenticated_transport_heals_recovery() {
        let (mut client, mut server) = establish_tunn_pair();
        let mut buffer = [0u8; 2048];
        for payload in [Vec::new(), ipv4_packet_84_bytes().to_vec()] {
            let packet = match server.encapsulate(&payload, &mut buffer) {
                TunnResult::WriteToNetwork(packet) => packet.to_vec(),
                other => panic!("{other:?}"),
            };
            let mut tampered = packet.clone();
            *tampered.last_mut().unwrap() ^= 1;
            let result = client.decapsulate(None, &tampered, &mut buffer);
            assert!(!authenticated_receive(&tampered, &result));
            let result = client.decapsulate(None, &packet, &mut buffer);
            assert!(authenticated_receive(&packet, &result));
            let result = client.decapsulate(None, &packet, &mut buffer);
            assert!(
                !authenticated_receive(&packet, &result),
                "replay must not heal recovery"
            );
        }
        assert!(!authenticated_receive(&[], &TunnResult::Done));
        assert!(!authenticated_receive(&[3, 0, 0, 0], &TunnResult::Done));
    }

    #[tokio::test]
    async fn recovery_rebinds_udp_and_can_complete_a_fresh_handshake() {
        let receiver = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let old = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        old.connect(receiver.local_addr().unwrap()).await.unwrap();
        let new = renewed_udp(&old).await.unwrap();
        assert_ne!(
            old.local_addr().unwrap().port(),
            new.local_addr().unwrap().port()
        );
        assert_eq!(old.peer_addr().unwrap(), new.peer_addr().unwrap());
        let (mut client, mut server) = establish_tunn_pair();
        let mut buffer = [0u8; 2048];
        // 丢弃一次握手请求，恢复必须能重新生成有效请求。
        assert!(matches!(
            client.format_handshake_initiation(&mut buffer, true),
            TunnResult::WriteToNetwork(_)
        ));
        let request = match client.format_handshake_initiation(&mut buffer, true) {
            TunnResult::WriteToNetwork(packet) => packet.to_vec(),
            other => panic!("{other:?}"),
        };
        new.send(&request).await.unwrap();
        let (n, from) =
            tokio::time::timeout(Duration::from_secs(1), receiver.recv_from(&mut buffer))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(from.port(), new.local_addr().unwrap().port());
        let received = buffer[..n].to_vec();
        let response = match server.decapsulate(None, &received, &mut buffer) {
            TunnResult::WriteToNetwork(packet) => packet.to_vec(),
            other => panic!("{other:?}"),
        };
        let mut recovery = TunnelRecovery::default();
        let now = Duration::from_secs(15);
        assert_eq!(recovery.tick(now, false), (false, true));
        // 故意不推进 boringtun 定时器：短 RTT 响应的握手时间仍是旧 tick。
        let result = client.decapsulate(None, &response, &mut buffer);
        assert!(authenticated_receive(&response, &result));
        recovery.authenticated_received(now);
        assert_eq!(recovery.tick(now, false), (true, false));
        assert_eq!(
            recovery.tick(now + Duration::from_secs(60), false),
            (true, false)
        );
    }

    fn establish_tunn_pair() -> (Tunn, Tunn) {
        let client_secret = StaticSecret::from([1u8; 32]);
        let client_public = PublicKey::from(&client_secret);
        let server_secret = StaticSecret::from([2u8; 32]);
        let server_public = PublicKey::from(&server_secret);
        let mut client = Tunn::new(client_secret, server_public, None, None, 1, None);
        let mut server = Tunn::new(server_secret, client_public, None, None, 2, None);
        let mut buffer = vec![0u8; 2048];

        let initiation = match client.encapsulate(&[], &mut buffer) {
            TunnResult::WriteToNetwork(packet) => packet.to_vec(),
            result => panic!("expected handshake initiation, got {result:?}"),
        };
        let response = match server.decapsulate(None, &initiation, &mut buffer) {
            TunnResult::WriteToNetwork(packet) => packet.to_vec(),
            result => panic!("expected handshake response, got {result:?}"),
        };
        let keepalive = match client.decapsulate(None, &response, &mut buffer) {
            TunnResult::WriteToNetwork(packet) => packet.to_vec(),
            result => panic!("expected keepalive, got {result:?}"),
        };
        assert!(matches!(
            server.decapsulate(None, &keepalive, &mut buffer),
            TunnResult::Done
        ));
        (client, server)
    }

    fn ipv4_packet_84_bytes() -> Vec<u8> {
        let mut packet = vec![0u8; 84];
        packet[0] = 0x45;
        packet[2..4].copy_from_slice(&(84u16).to_be_bytes());
        packet[8] = 64;
        packet[9] = 1;
        packet[12..16].copy_from_slice(&[10, 9, 0, 2]);
        packet[16..20].copy_from_slice(&[10, 9, 0, 1]);
        packet
    }

    #[tokio::test]
    async fn outgoing_vpn_helper_sends_original_bytes_and_counts_plaintext_once() {
        let receiver = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        sender
            .connect(receiver.local_addr().unwrap())
            .await
            .unwrap();
        let (mut client, mut server) = establish_tunn_pair();
        // TCP, UDP and ICMP all retain their original bytes through the same
        // WireGuard path, including its padding and traffic accounting.
        for protocol in [6, 17, 1] {
            let mut original = ipv4_packet_84_bytes();
            original[9] = protocol;
            let mut plaintext = [0xa5; 128];
            plaintext[..original.len()].copy_from_slice(&original);
            let mut encrypted = [0; 2048];
            let mut send_state = SendState::default();

            let (bytes, failures) = encapsulate_outgoing(
                &mut client,
                &sender,
                None,
                &mut plaintext,
                original.len(),
                &mut encrypted,
                &mut send_state,
            )
            .await
            .unwrap();
            assert_eq!((bytes, failures), (original.len() as u64, 0));
            assert!(send_state.pending_tx_lengths.is_empty());
            let mut received = [0; 2048];
            let size = tokio::time::timeout(Duration::from_secs(1), receiver.recv(&mut received))
                .await
                .unwrap()
                .unwrap();
            match server.decapsulate(None, &received[..size], &mut encrypted) {
                TunnResult::WriteToTunnelV4(packet, _) => assert_eq!(&*packet, original.as_slice()),
                result => panic!("expected original VPN packet, got {result:?}"),
            }
        }
    }

    #[tokio::test]
    async fn outgoing_vpn_helper_does_not_count_failed_udp_data_as_sent() {
        // An unconnected UDP socket makes send fail locally, without contacting
        // any external peer or modifying host routes.
        let sender = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let (mut client, _) = establish_tunn_pair();
        let original = ipv4_packet_84_bytes();
        let mut plaintext = [0; 128];
        plaintext[..original.len()].copy_from_slice(&original);
        let mut encrypted = [0; 2048];
        let mut send_state = SendState::default();

        assert_eq!(
            encapsulate_outgoing(
                &mut client,
                &sender,
                None,
                &mut plaintext,
                original.len(),
                &mut encrypted,
                &mut send_state,
            )
            .await
            .unwrap(),
            (0, 1)
        );
        assert!(send_state.pending_tx_lengths.is_empty());
    }

    #[tokio::test]
    async fn outgoing_vpn_helper_tracks_data_queued_behind_handshake() {
        let receiver = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        sender
            .connect(receiver.local_addr().unwrap())
            .await
            .unwrap();
        let server_public = PublicKey::from(&StaticSecret::from([2u8; 32]));
        let mut client = Tunn::new(
            StaticSecret::from([1u8; 32]),
            server_public,
            None,
            None,
            1,
            None,
        );
        let original = ipv4_packet_84_bytes();
        let mut plaintext = [0; 128];
        plaintext[..original.len()].copy_from_slice(&original);
        let mut encrypted = [0; 2048];
        let mut send_state = SendState::default();

        assert_eq!(
            encapsulate_outgoing(
                &mut client,
                &sender,
                None,
                &mut plaintext,
                original.len(),
                &mut encrypted,
                &mut send_state,
            )
            .await
            .unwrap(),
            (0, 0)
        );
        assert_eq!(
            send_state.pending_tx_lengths,
            VecDeque::from([original.len() as u64])
        );
        let mut received = [0; 2048];
        let size = tokio::time::timeout(Duration::from_secs(1), receiver.recv(&mut received))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(wireguard_packet_type(&received[..size]), Some(1));
    }

    #[test]
    fn padded_ipv4_packet_round_trips_through_boringtun_and_both_obfs_modes() {
        for mode in [Mode::LowOverheadV1, Mode::ParanoidV1] {
            let (mut client, mut server) = establish_tunn_pair();
            let original = ipv4_packet_84_bytes();
            let mut plaintext = vec![0u8; 96];
            plaintext[..original.len()].copy_from_slice(&original);
            let padded = pad_wireguard_plaintext(&mut plaintext, original.len()).unwrap();
            let mut network_buffer = vec![0u8; 2048];
            let wireguard = match client.encapsulate(padded, &mut network_buffer) {
                TunnResult::WriteToNetwork(packet) => packet.to_vec(),
                result => panic!("expected WireGuard data, got {result:?}"),
            };

            assert_eq!(wireguard.len(), 128);
            assert_eq!(vpn_obfs::validate_wireguard(&wireguard, 1472), Ok(4));

            let psk = [7u8; 32];
            let encoder = Codec::new(&psk, mode, Direction::ClientToServer, 1472).unwrap();
            let decoder = Codec::new(&psk, mode, Direction::ClientToServer, 1472).unwrap();
            let encoded = encoder.encode(&wireguard).unwrap();
            let decoded = decoder.decode(&encoded, &mut ReplayCache::new()).unwrap();
            let decrypted = match server.decapsulate(None, &decoded, &mut network_buffer) {
                TunnResult::WriteToTunnelV4(packet, _) => packet.to_vec(),
                result => panic!("expected IPv4 packet, got {result:?}"),
            };

            assert_eq!(decrypted.len(), original.len());
            assert_eq!(decrypted, original);
        }
    }
}
