//! Rust 原生 WireGuard UDP 混淆代理。

use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Context;
use socket2::SockRef;
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;
use vpn_obfs::{Codec, Direction, Mode, ReplayCache};

use crate::config::ObfsConfig;

struct Session {
    wg_socket: Arc<UdpSocket>,
    activity: Arc<Mutex<Instant>>,
    response_task: JoinHandle<()>,
}

impl Drop for Session {
    fn drop(&mut self) {
        self.response_task.abort();
    }
}

#[derive(Debug, Default)]
struct SessionLimiter {
    attempts: HashMap<IpAddr, VecDeque<Instant>>,
}

impl SessionLimiter {
    fn allow(&mut self, ip: IpAddr, now: Instant, limit: u32) -> bool {
        let attempts = self.attempts.entry(ip).or_default();
        while attempts
            .front()
            .is_some_and(|seen| now.duration_since(*seen) >= Duration::from_secs(60))
        {
            attempts.pop_front();
        }
        if attempts.len() >= limit as usize {
            return false;
        }
        attempts.push_back(now);
        true
    }

    fn prune(&mut self, now: Instant) {
        self.attempts.retain(|_, attempts| {
            while attempts
                .front()
                .is_some_and(|seen| now.duration_since(*seen) >= Duration::from_secs(60))
            {
                attempts.pop_front();
            }
            !attempts.is_empty()
        });
    }
}

/// 已绑定公网 socket 的混淆服务。
pub struct UdpObfsServer {
    config: ObfsConfig,
    public_socket: Arc<UdpSocket>,
    wg_endpoint: SocketAddr,
    decoder: Codec,
    encoder: Codec,
}

impl UdpObfsServer {
    /// 绑定公网 UDP 端口并初始化方向隔离的编解码器。
    pub async fn bind(config: ObfsConfig, wg_port: u16) -> anyhow::Result<Self> {
        let public_socket = Arc::new(
            UdpSocket::bind(&config.bind_addr)
                .await
                .with_context(|| format!("绑定混淆 UDP 地址 {} 失败", config.bind_addr))?,
        );
        configure_udp_buffers(&public_socket, "public");
        let mode = map_mode(config.mode);
        let ip_udp_overhead = if public_socket.local_addr()?.is_ipv6() {
            48
        } else {
            28
        };
        let max_datagram = usize::from(config.path_mtu)
            .checked_sub(ip_udp_overhead)
            .context("混淆 path MTU 太小")?;
        let decoder = Codec::new(
            config.psk.as_ref(),
            mode,
            Direction::ClientToServer,
            max_datagram,
        )?;
        let encoder = Codec::new(
            config.psk.as_ref(),
            mode,
            Direction::ServerToClient,
            max_datagram,
        )?;
        Ok(Self {
            config,
            public_socket,
            wg_endpoint: SocketAddr::from(([127, 0, 0, 1], wg_port)),
            decoder,
            encoder,
        })
    }

    /// 返回实际绑定地址（测试和启动诊断使用）。
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.public_socket.local_addr()
    }

    /// 运行代理直到任务被取消或公网 socket 发生致命错误。
    pub async fn run(self) -> anyhow::Result<()> {
        tracing::info!(
            stage = "obfs_listen",
            result = "started",
            mode = ?self.config.mode,
            bind = %self.config.bind_addr,
            max_sessions = self.config.max_sessions,
            "UDP 混淆代理已启动"
        );
        let mut sessions: HashMap<SocketAddr, Session> = HashMap::new();
        let mut limiter = SessionLimiter::default();
        let mut replay = ReplayCache::new();
        let mut cleanup = tokio::time::interval(Duration::from_secs(10));
        let mut stats = DropStats::default();
        let mut buf = vec![0u8; usize::from(self.config.path_mtu) + 64];

        loop {
            tokio::select! {
                received = self.public_socket.recv_from(&mut buf) => {
                    let (len, source) = received.context("接收混淆 UDP 数据报失败")?;
                    let packet = match self.decoder.decode(&buf[..len], &mut replay) {
                        Ok(packet) => packet,
                        Err(error) => {
                            stats.record(&error);
                            continue;
                        }
                    };
                    let now = Instant::now();
                    if let Some(session) = sessions.get_mut(&source) {
                        *session.activity.lock().unwrap_or_else(|error| error.into_inner()) = now;
                        if session.wg_socket.send(&packet).await.is_err() {
                            stats.internal = stats.internal.saturating_add(1);
                            sessions.remove(&source);
                        }
                        continue;
                    }
                    if sessions.len() >= self.config.max_sessions {
                        stats.capacity = stats.capacity.saturating_add(1);
                        continue;
                    }
                    if !limiter.allow(source.ip(), now, self.config.new_sessions_per_ip_per_minute) {
                        stats.rate_limited = stats.rate_limited.saturating_add(1);
                        continue;
                    }
                    let wg_socket = match bind_internal(self.wg_endpoint).await {
                        Ok(socket) => Arc::new(socket),
                        Err(error) => {
                            stats.internal = stats.internal.saturating_add(1);
                            tracing::warn!(stage = "obfs_session", result = "failed", error = %error, "创建内部 WG 会话失败");
                            continue;
                        }
                    };
                    if wg_socket.send(&packet).await.is_err() {
                        stats.internal = stats.internal.saturating_add(1);
                        continue;
                    }
                    let activity = Arc::new(Mutex::new(now));
                    let response_task = spawn_response_loop(
                        wg_socket.clone(),
                        self.public_socket.clone(),
                        source,
                        self.encoder.clone(),
                        activity.clone(),
                    );
                    sessions.insert(source, Session { wg_socket, activity, response_task });
                    tracing::info!(stage = "obfs_session", result = "created", client = %source, active = sessions.len(), "混淆代理会话已创建");
                }
                _ = cleanup.tick() => {
                    let now = Instant::now();
                    let idle = Duration::from_secs(self.config.session_idle_secs);
                    let stale = sessions.iter()
                        .filter_map(|(source, session)| {
                            let last_seen = *session.activity.lock().unwrap_or_else(|error| error.into_inner());
                            (now.duration_since(last_seen) >= idle || session.response_task.is_finished()).then_some(*source)
                        })
                        .collect::<Vec<_>>();
                    for source in stale {
                        if sessions.remove(&source).is_some() {
                            tracing::info!(stage = "obfs_session", result = "expired", client = %source, active = sessions.len(), "混淆代理会话已回收");
                        }
                    }
                    limiter.prune(now);
                    stats.log_and_reset(sessions.len());
                }
            }
        }
    }
}

// 每个 socket 单独设置，避免依赖宿主机默认值；内核可能按系统上限裁剪。
const UDP_SOCKET_BUFFER_BYTES: usize = 8 * 1024 * 1024;

fn configure_udp_buffers(socket: &UdpSocket, role: &'static str) {
    let socket = SockRef::from(socket);
    let configure = || -> std::io::Result<(usize, usize)> {
        if socket.recv_buffer_size()? < UDP_SOCKET_BUFFER_BYTES {
            socket.set_recv_buffer_size(UDP_SOCKET_BUFFER_BYTES)?;
        }
        if socket.send_buffer_size()? < UDP_SOCKET_BUFFER_BYTES {
            socket.set_send_buffer_size(UDP_SOCKET_BUFFER_BYTES)?;
        }
        Ok((socket.recv_buffer_size()?, socket.send_buffer_size()?))
    };
    match configure() {
        Ok((recv_bytes, send_bytes)) => {
            if recv_bytes < UDP_SOCKET_BUFFER_BYTES || send_bytes < UDP_SOCKET_BUFFER_BYTES {
                tracing::warn!(
                    role,
                    recv_bytes,
                    send_bytes,
                    requested_bytes = UDP_SOCKET_BUFFER_BYTES,
                    "混淆 UDP 缓冲区受系统上限限制"
                );
            } else {
                tracing::debug!(role, recv_bytes, send_bytes, "混淆 UDP 缓冲区已设置");
            }
        }
        Err(error) => tracing::warn!(role, %error, "设置混淆 UDP 缓冲区失败，继续使用可用缓冲区"),
    }
}

async fn bind_internal(endpoint: SocketAddr) -> anyhow::Result<UdpSocket> {
    let bind = if endpoint.is_ipv6() {
        "[::1]:0"
    } else {
        "127.0.0.1:0"
    };
    let socket = UdpSocket::bind(bind)
        .await
        .context("创建内部 WG socket 失败")?;
    configure_udp_buffers(&socket, "internal");
    socket
        .connect(endpoint)
        .await
        .context("连接内部 WG socket 失败")?;
    Ok(socket)
}

fn spawn_response_loop(
    wg_socket: Arc<UdpSocket>,
    public_socket: Arc<UdpSocket>,
    source: SocketAddr,
    encoder: Codec,
    activity: Arc<Mutex<Instant>>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut buf = vec![0u8; 65535];
        loop {
            let len = match wg_socket.recv(&mut buf).await {
                Ok(len) => len,
                Err(error) => {
                    tracing::warn!(stage = "obfs_internal_recv", result = "failed", client = %source, error = %error, "内部 WG socket 读取失败");
                    break;
                }
            };
            let encoded = match encoder.encode(&buf[..len]) {
                Ok(encoded) => encoded,
                Err(error) => {
                    tracing::warn!(stage = "obfs_internal_encode", result = "dropped", client = %source, error = %error, "内部 WG 响应格式不合法");
                    continue;
                }
            };
            match public_socket.send_to(&encoded, source).await {
                Ok(_) => {
                    *activity.lock().unwrap_or_else(|error| error.into_inner()) = Instant::now();
                }
                Err(error) => {
                    tracing::warn!(stage = "obfs_public_send", result = "failed", client = %source, error = %error, "发送混淆响应失败");
                }
            }
        }
    })
}

fn map_mode(mode: vpn_api_types::peer::ObfsMode) -> Mode {
    match mode {
        vpn_api_types::peer::ObfsMode::LowOverheadV1 => Mode::LowOverheadV1,
        vpn_api_types::peer::ObfsMode::ParanoidV1 => Mode::ParanoidV1,
    }
}

#[derive(Debug, Default)]
struct DropStats {
    invalid: u64,
    replay: u64,
    clock: u64,
    rate_limited: u64,
    capacity: u64,
    internal: u64,
}

impl DropStats {
    fn record(&mut self, error: &vpn_obfs::ObfsError) {
        match error {
            vpn_obfs::ObfsError::Replay => self.replay = self.replay.saturating_add(1),
            vpn_obfs::ObfsError::ClockSkew => self.clock = self.clock.saturating_add(1),
            _ => self.invalid = self.invalid.saturating_add(1),
        }
    }

    fn log_and_reset(&mut self, sessions: usize) {
        if self.invalid
            + self.replay
            + self.clock
            + self.rate_limited
            + self.capacity
            + self.internal
            > 0
        {
            tracing::warn!(
                stage = "obfs_drop_summary",
                invalid = self.invalid,
                replay = self.replay,
                clock = self.clock,
                rate_limited = self.rate_limited,
                capacity = self.capacity,
                internal = self.internal,
                sessions,
                "UDP 混淆丢弃统计"
            );
        }
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vpn_api_types::peer::ObfsMode;
    use vpn_obfs::{Direction, ReplayCache};
    use zeroize::Zeroizing;

    #[test]
    fn limiter_allows_twenty_new_sessions_per_minute() {
        let mut limiter = SessionLimiter::default();
        let now = Instant::now();
        let ip = IpAddr::from([192, 0, 2, 1]);
        for _ in 0..20 {
            assert!(limiter.allow(ip, now, 20));
        }
        assert!(!limiter.allow(ip, now, 20));
        assert!(limiter.allow(ip, now + Duration::from_secs(60), 20));
    }

    #[tokio::test]
    async fn distinct_outer_addresses_get_independent_wireguard_sockets() {
        let wg = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let wg_port = wg.local_addr().unwrap().port();
        let config = ObfsConfig {
            bind_addr: "127.0.0.1:0".into(),
            public_endpoint: "127.0.0.1:0".into(),
            mode: ObfsMode::LowOverheadV1,
            path_mtu: 1500,
            psk: Zeroizing::new([11u8; 32]),
            max_sessions: 4096,
            new_sessions_per_ip_per_minute: 20,
            session_idle_secs: 180,
        };
        let server = UdpObfsServer::bind(config, wg_port).await.unwrap();
        let proxy_addr = server.local_addr().unwrap();
        let server_task = tokio::spawn(server.run());
        let wg_task = tokio::spawn(async move {
            let mut sources = Vec::new();
            let mut buf = [0u8; 256];
            for _ in 0..2 {
                let (len, source) = wg.recv_from(&mut buf).await.unwrap();
                sources.push(source);
                wg.send_to(&buf[..len], source).await.unwrap();
            }
            sources
        });

        let encoder = Codec::new(
            &[11u8; 32],
            Mode::LowOverheadV1,
            Direction::ClientToServer,
            1472,
        )
        .unwrap();
        let decoder = Codec::new(
            &[11u8; 32],
            Mode::LowOverheadV1,
            Direction::ServerToClient,
            1472,
        )
        .unwrap();
        let mut wg_packet = vec![0u8; 148];
        wg_packet[0] = 1;
        for _ in 0..2 {
            let client = UdpSocket::bind("127.0.0.1:0").await.unwrap();
            client.connect(proxy_addr).await.unwrap();
            client
                .send(&encoder.encode(&wg_packet).unwrap())
                .await
                .unwrap();
            let mut response = vec![0u8; 2048];
            let len = tokio::time::timeout(Duration::from_secs(2), client.recv(&mut response))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                decoder
                    .decode(&response[..len], &mut ReplayCache::new())
                    .unwrap(),
                wg_packet
            );
        }
        let sources = wg_task.await.unwrap();
        assert_ne!(sources[0], sources[1]);
        server_task.abort();
    }
}
