//! 用户态 WireGuard 数据面：boringtun(协议栈) + tun(TUN 设备) + UDP，**零外部依赖**。
//!
//! 不再 shell-out 到 `wg`/`wg-quick`/`wireguard.exe`，整条隧道在进程内完成：
//! - [`boringtun::noise::Tunn`]：握手 + 加解密 + keepalive 状态机；
//! - `tun` crate：跨平台 TUN 设备（Linux `/dev/net/tun`、macOS `utun`、Windows WinTun）；
//! - `net-route`：跨平台路由表(为 allowed_ips 加 `dev <tun>` 路由)；
//! - tokio UDP：与服务端 endpoint 收发密文。
//!
//! 转发循环（单任务 `tokio::select!`）：
//! - TUN 读到出站 IP 包 → `Tunn::encapsulate` → UDP 送服务端；
//! - UDP 收到密文 → `Tunn::decapsulate` → 写回 TUN（或回送握手包）；
//! - 定时 `Tunn::update_timers` → 维护握手 / persistent-keepalive。
//!
//! 运行要求：仅需 root/管理员（开 TUN 设备），**无需安装任何 WireGuard 工具**。
//! Windows 额外需随包分发的 `wintun.dll`（由 `tun` 依赖加载，非用户安装）。

use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use base64::Engine;
use boringtun::noise::{Tunn, TunnResult};
use boringtun::x25519::{PublicKey, StaticSecret};
use net_route::{Handle, Route};
use tokio::net::UdpSocket;
use tokio::sync::watch;
use tracing::Instrument;
use tun::AbstractDevice;
use vpn_api_types::peer::ObfsMode;
use vpn_api_types::system::{obfs_transport_safe_mtu, NetworkMtuMode, NetworkSettings};
use vpn_obfs::{Codec, Direction, Mode, ReplayCache};
use zeroize::Zeroizing;

use crate::daemon::{SharedState, TunnelTransport};
use crate::error::{CliError, CliResult};

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
fn parse_cidr_v4(s: &str) -> Option<(Ipv4Addr, u8)> {
    let (ip, pfx) = s.trim().split_once('/')?;
    let ip: Ipv4Addr = ip.parse().ok()?;
    let pfx: u8 = pfx.parse().ok()?;
    if pfx > 32 {
        return None;
    }
    Some((ip, pfx))
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
        vpn_ip: Ipv4Addr,
        subnet_prefix: u8,
        allowed_routes: &[String],
        keepalive_secs: u16,
        shutdown: watch::Receiver<bool>,
        // 转发循环遇致命错误(如 TUN 读失败)时广播关停,连带停掉心跳任务,避免它继续
        // 上报 Connected 掩盖数据面已死。通常传 shutdown 对应的 Sender 的 clone。
        shutdown_tx: watch::Sender<bool>,
        // 流量计数回写目标（前端读 bytes_rx/bytes_tx）；None 时不统计。
        traffic: Option<SharedState>,
        // 实时路由更新：心跳检测到 allowed_routes 变化时推送新集合,转发循环据此增量
        // 增删本地路由(P1.4);None 时不支持热更新。
        routes_rx: Option<watch::Receiver<Vec<String>>>,
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

        // 5) 加路由：把所有 allowed_routes（含 VPN 子网）显式指向 TUN 接口。
        // 不能依赖"接口地址自动产生连通路由"——macOS utun 是点对点接口，不会自动生成
        // 子网路由（发往 10.8.0.1 的包会走默认网卡而非隧道）。Linux 上若连通路由已存在，
        // 这里的重复添加只会无害告警。
        let routes_started = std::time::Instant::now();
        tracing::info!(
            stage = "route_apply",
            result = "started",
            requested = allowed_routes.len(),
            "开始应用 VPN 路由"
        );
        let requested_routes = allowed_routes.len();
        let handle = Handle::new().map_err(|error| {
            tracing::warn!(stage = "route_apply", result = "failed", elapsed_ms = routes_started.elapsed().as_millis(), error = %crate::error::redact_sensitive(&error.to_string()), "创建路由句柄失败");
            CliError::Other(format!("路由句柄失败: {error}"))
        })?;
        // 以 (归一化 CIDR 串, Route) 记账,便于后续与心跳下发的新集合做增量 diff。
        let mut added: Vec<(String, Route)> = Vec::new();
        for r in allowed_routes {
            let Some((dest, pfx)) = parse_cidr_v4(r) else {
                tracing::warn!(route = %r, "跳过非法 allowed_route");
                continue;
            };
            if pfx == 0 {
                // 默认路由（0.0.0.0/0）缺少 endpoint 旁路会把发往服务端的 UDP 也卷进隧道形成
                // 回环、瘫痪连接。服务端已在路由校验处拒绝 0.0.0.0/0，这里再兜底跳过以防旧配置。
                tracing::warn!(route = %r, "跳过默认路由(0.0.0.0/0):用户态后端暂不支持全隧道");
                continue;
            }
            let route = Route::new(IpAddr::V4(dest), pfx).with_ifindex(ifindex);
            match handle.add(&route).await {
                Ok(()) => added.push((r.clone(), route)),
                Err(e) => tracing::warn!(
                    stage = "route_apply",
                    result = "failed",
                    route = %r,
                    error = %crate::error::redact_sensitive(&e.to_string()),
                    "加路由失败（可能已存在）"
                ),
            }
        }

        let route_result = if added.len() == requested_routes {
            "succeeded"
        } else if added.is_empty() && requested_routes > 0 {
            "failed"
        } else {
            "degraded"
        };
        tracing::info!(
            stage = "route_apply",
            result = route_result,
            elapsed_ms = routes_started.elapsed().as_millis(),
            requested = requested_routes,
            applied = added.len(),
            skipped_or_failed = requested_routes.saturating_sub(added.len()),
            "VPN 路由应用完成"
        );

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
    udp: UdpSocket,
    mut tunn: Tunn,
    handle: Handle,
    mut added_routes: Vec<(String, Route)>,
    ifindex: u32,
    mut shutdown: watch::Receiver<bool>,
    shutdown_tx: watch::Sender<bool>,
    traffic: Option<SharedState>,
    mut routes_rx: Option<watch::Receiver<Vec<String>>>,
    mut obfs: Option<ObfsRuntime>,
    packet_buffer_size: usize,
) -> CliResult<()> {
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
            // 出站：TUN → 加密 → UDP
            r = device.recv(&mut tun_read_buf) => {
                match r {
                    Ok(n) => {
                        if n == 0 {
                            continue;
                        }
                        let plaintext = match pad_wireguard_plaintext(&mut tun_read_buf, n) {
                            Ok(packet) => packet,
                            Err(error) => {
                                tracing::warn!(
                                    stage = "wireguard_padding",
                                    result = "failed",
                                    original_len = n,
                                    buffer_capacity = tun_read_buf.len(),
                                    error = %error.safe_diagnostic(),
                                    "WireGuard 业务报文填充失败，数据面停止"
                                );
                                if let Some(s) = &traffic {
                                    s.set_error(
                                        format!("数据面中断(WireGuard 填充失败): {}", error.safe_diagnostic()),
                                        crate::daemon::now_unix(),
                                    ).await;
                                }
                                let _ = shutdown_tx.send(true);
                                break Err(error);
                            }
                        };
                        match tunn.encapsulate(plaintext, &mut enc_buf) {
                            TunnResult::WriteToNetwork(p) => {
                                let is_data = wireguard_packet_type(p) == Some(4);
                                let sent = match send_network(&udp, obfs.as_ref(), p).await {
                                    Ok(()) => true,
                                    Err(error) => {
                                        send_state.failure_logger.record("tun_data", p, &error);
                                        udp_send_failures = udp_send_failures.saturating_add(1);
                                        false
                                    }
                                };
                                if is_data && sent {
                                    // 只在业务 Data 报文实际发送成功后统计；握手包不算用户流量。
                                    tx_acc = tx_acc.saturating_add(n as u64);
                                } else if !is_data {
                                    send_state.track_pending_tx(n);
                                }
                            }
                            TunnResult::Done => {
                                send_state.track_pending_tx(n);
                            }
                            TunnResult::Err(error) => {
                                tracing::warn!(
                                    stage = "wireguard_encapsulate",
                                    result = "failed",
                                    original_len = n,
                                    padded_len = plaintext.len(),
                                    error = ?error,
                                    "WireGuard 业务报文封装失败"
                                );
                            }
                            _ => {}
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
                            Ok((rx_bytes, tx_bytes, send_failures)) => {
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
            // 实时路由更新：心跳检测到 allowed_routes 变化 → 增量增删本地路由。
            changed = wait_routes_change(&mut routes_rx) => {
                if changed {
                    if let Some(rx) = routes_rx.as_ref() {
                        let desired = rx.borrow().clone();
                        apply_route_diff(&handle, ifindex, &mut added_routes, &desired).await;
                    }
                }
            }
            // 定时器：握手重传 / keepalive，并顺带把累计流量刷回状态。
            _ = ticker.tick() => {
                let mut tbuf = vec![0u8; packet_buffer_size];
                match tunn.update_timers(&mut tbuf) {
                    TunnResult::WriteToNetwork(p) => {
                        if let Err(error) = send_network(&udp, obfs.as_ref(), p).await {
                            send_state.failure_logger.record("timer", p, &error);
                            udp_send_failures = udp_send_failures.saturating_add(1);
                        }
                    }
                    TunnResult::Err(error) => {
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

    // 清理：删除本任务加的路由（TUN 设备随 device drop 关闭）。
    let mut cleanup_failures = 0;
    for (cidr, route) in &added_routes {
        if let Err(error) = handle.delete(route).await {
            cleanup_failures += 1;
            tracing::warn!(route = %cidr, error = %error, "删除 VPN 路由失败");
        }
    }
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
            "VPN 路由清理失败: {cleanup_failures} 条"
        )));
    }
    outcome
}

/// 等待路由更新通道有新值;通道为 None(不支持热更新)时永不就绪——该 select 分支不触发。
async fn wait_routes_change(rx: &mut Option<watch::Receiver<Vec<String>>>) -> bool {
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

/// 把本地路由表增量对齐到 `desired`:删除已不在集合的、新增缺少的(按 CIDR 串比对,
/// 顺序无关)。VPN 子网与各站点网段都恒在 desired 内 → 保持;仅组/服务端网段的增减
/// 被实际增删。net-route 操作 best-effort,失败仅告警不致命。
async fn apply_route_diff(
    handle: &Handle,
    ifindex: u32,
    added: &mut Vec<(String, Route)>,
    desired: &[String],
) {
    // 1) 删除不再需要的。
    let mut keep: Vec<(String, Route)> = Vec::with_capacity(added.len());
    for (cidr, route) in std::mem::take(added) {
        if desired.iter().any(|d| d == &cidr) {
            keep.push((cidr, route));
        } else {
            let _ = handle.delete(&route).await;
            tracing::info!(route = %cidr, "allowed_routes 变更:移除路由");
        }
    }
    *added = keep;
    // 2) 新增缺少的。
    for d in desired {
        if added.iter().any(|(c, _)| c == d) {
            continue;
        }
        let Some((dest, pfx)) = parse_cidr_v4(d) else {
            continue;
        };
        if pfx == 0 {
            tracing::warn!(route = %d, "跳过默认路由(0.0.0.0/0):用户态后端暂不支持全隧道");
            continue;
        }
        let route = Route::new(IpAddr::V4(dest), pfx).with_ifindex(ifindex);
        match handle.add(&route).await {
            Ok(()) => {
                added.push((d.clone(), route));
                tracing::info!(route = %d, "allowed_routes 变更:新增路由");
            }
            Err(e) => tracing::warn!(
                stage = "route_update",
                result = "failed",
                route = %d,
                error = %crate::error::redact_sensitive(&e.to_string()),
                "新增路由失败(可能已存在)"
            ),
        }
    }
}

/// 处理一个入站 UDP 数据报：解密后写回 TUN；握手响应回送网络；并排空队列。
///
/// 返回写回 TUN 的明文字节数（用于流量统计）；握手 / keepalive / 错误返回 0。
async fn handle_incoming(
    tunn: &mut Tunn,
    udp: &UdpSocket,
    device: &tun::AsyncDevice,
    packet: &[u8],
    obfs: Option<&ObfsRuntime>,
    packet_buffer_size: usize,
    send_state: &mut SendState,
) -> CliResult<(u64, u64, u64)> {
    let mut out = vec![0u8; packet_buffer_size];
    match tunn.decapsulate(None, packet, &mut out) {
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
            Ok((0, tx_bytes, send_failures))
        }
        TunnResult::WriteToTunnelV4(p, _) | TunnResult::WriteToTunnelV6(p, _) => {
            let n = p.len() as u64;
            device
                .send(p)
                .await
                .map_err(|e| CliError::Other(format!("写入 TUN 失败: {e}")))?;
            Ok((n, 0, 0))
        }
        TunnResult::Done => Ok((0, 0, 0)),
        TunnResult::Err(e) => {
            tracing::debug!(?e, "decapsulate 错误（忽略单包）");
            Ok((0, 0, 0))
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
