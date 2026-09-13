//! Select the path once, before the application's first ordinary IPv4 TCP SYN.
//!
//! A successful physical connect is the upstream business socket. smoltcp then
//! terminates the TUN-side TCP connection and relays opaque bytes. No business
//! bytes are retried through WireGuard after this commitment. Queues, concurrent
//! work, and decision history are bounded; saturation selects VPN for new flows.

use crate::{
    direct_dial::DirectDialer,
    tcp_stack::{FlowKey, TcpStack},
};
use async_trait::async_trait;
use ipnet::Ipv4Net;
use std::{
    collections::{HashMap, HashSet},
    io,
    net::{Ipv4Addr, SocketAddrV4},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{mpsc, watch},
    task::{AbortHandle, JoinHandle, JoinSet},
    time::Instant,
};

const QUEUE: usize = 256;
const MAX_PENDING: usize = 16;
const MAX_DIRECT: usize = 256;
// Never forget a committed tuple or an attempted SYN during this tunnel session.
// Reaching this limit only disables new direct decisions, not existing relays.
const MAX_HISTORY: usize = 65_536;
const REUSE_DELAY: Duration = Duration::from_secs(120);
const SYN: u8 = 2;
const RST: u8 = 4;
const ACK: u8 = 16;

pub(crate) enum Event {
    ToVpn(Vec<u8>),
    ToTun(Vec<u8>),
    Traffic { tx: u64, rx: u64 },
}

trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}
type ByteStream = Box<dyn Stream>;

#[async_trait]
trait Connector: Send + Sync {
    async fn connect(&self, target: SocketAddrV4) -> io::Result<ByteStream>;
    async fn shutdown(&self) -> io::Result<()> {
        Ok(())
    }
}

#[async_trait]
impl Connector for DirectDialer {
    async fn connect(&self, target: SocketAddrV4) -> io::Result<ByteStream> {
        Ok(Box::new(DirectDialer::connect(self, target).await?))
    }
    async fn shutdown(&self) -> io::Result<()> {
        DirectDialer::shutdown(self).await
    }
}

struct UnavailableConnector(String);
#[async_trait]
impl Connector for UnavailableConnector {
    async fn connect(&self, _: SocketAddrV4) -> io::Result<ByteStream> {
        Err(io::Error::other(self.0.clone()))
    }
}

pub(crate) struct Router {
    packets: mpsc::Sender<Vec<u8>>,
    allowed: watch::Sender<Policy>,
    stop: watch::Sender<bool>,
    output: mpsc::Receiver<Event>,
    runner: Option<JoinHandle<io::Result<()>>>,
}

impl Router {
    pub fn start(allowed: Vec<String>, vpn: Ipv4Net, ifindex: u32, mtu: usize) -> Self {
        Self::spawn(
            allowed,
            vpn,
            mtu,
            DirectDialer::new(ifindex, vpn).map(|dialer| Arc::new(dialer) as Arc<dyn Connector>),
        )
    }

    fn spawn(
        allowed: Vec<String>,
        vpn: Ipv4Net,
        mtu: usize,
        connector: io::Result<Arc<dyn Connector>>,
    ) -> Self {
        let (packets, input) = mpsc::channel(QUEUE);
        let (allowed, allowed_rx) = watch::channel(Policy {
            routes: allowed,
            generation: 0,
        });
        let (stop, stop_rx) = watch::channel(false);
        let (events, output) = mpsc::channel(QUEUE);
        let runner = tokio::spawn(async move {
            let connector = connector.unwrap_or_else(|error| {
                tracing::warn!(stage = "lan_tcp_init", error = %crate::error::redact_sensitive(&error.to_string()), "内网直连暂不可用，保持 VPN 转发");
                Arc::new(UnavailableConnector(error.to_string()))
            });
            run(input, allowed_rx, stop_rx, events, connector, vpn, mtu).await
        });
        Self {
            packets,
            allowed,
            stop,
            output,
            runner: Some(runner),
        }
    }

    /// IPv4 TCP is owned by this dispatcher even when its input queue is full.
    /// Dropping a congested packet is safe for TCP; sending it on another path is not.
    pub fn try_send(&self, packet: Vec<u8>) -> bool {
        if !is_ipv4_tcp(&packet) {
            return false;
        }
        let _ = self.packets.try_send(packet);
        true
    }

    pub fn set_allowed(&self, allowed: Vec<String>) {
        self.allowed.send_modify(|policy| {
            policy.routes = allowed;
            policy.generation += 1;
        });
    }
    pub async fn recv(&mut self) -> Option<Event> {
        self.output.recv().await
    }

    pub async fn shutdown(mut self) -> io::Result<()> {
        let _ = self.stop.send(true);
        self.output.close(); // unblock a runner waiting to publish a packet
        if let Some(runner) = self.runner.take() {
            runner.await.map_err(io::Error::other)??;
        }
        Ok(())
    }
}

impl Drop for Router {
    fn drop(&mut self) {
        // Let the worker finish its asynchronous policy/stream cleanup even when
        // the owner exits unexpectedly. Explicit shutdown also joins the worker.
        let _ = self.stop.send(true);
    }
}

#[derive(Clone)]
struct Policy {
    routes: Vec<String>,
    generation: u64,
}

struct Flow {
    isn: u32,
    id: u64,
    state: State,
}
enum State {
    Pending { syn: Vec<u8>, abort: AbortHandle },
    Direct { abort: AbortHandle },
    Closed { reusable_after: Instant },
    Vpn,
}

struct Decisions {
    flows: HashMap<FlowKey, Flow>,
    attempted: HashSet<(FlowKey, u32)>,
    // A midstream or unsupported connection must not later be mistaken for new.
    vpn_only: HashSet<FlowKey>,
    fragmented_pairs: HashSet<(Ipv4Addr, Ipv4Addr)>,
    saturated: bool,
    next_id: u64,
}

impl Decisions {
    fn new() -> Self {
        Self {
            flows: HashMap::new(),
            attempted: HashSet::new(),
            vpn_only: HashSet::new(),
            fragmented_pairs: HashSet::new(),
            saturated: false,
            next_id: 0,
        }
    }
    fn fresh(&mut self, key: FlowKey, isn: u32) -> bool {
        if self.saturated || self.attempted.len() >= MAX_HISTORY {
            self.saturated = true;
            return false;
        }
        self.attempted.insert((key, isn))
    }
    fn pin_vpn(&mut self, key: FlowKey) {
        if self.vpn_only.len() >= MAX_HISTORY {
            self.saturated = true;
        } else {
            self.vpn_only.insert(key);
        }
    }
    fn active(&self) -> usize {
        self.flows
            .values()
            .filter(|f| matches!(f.state, State::Pending { .. } | State::Direct { .. }))
            .count()
    }
    fn pending(&self) -> usize {
        self.flows
            .values()
            .filter(|f| matches!(f.state, State::Pending { .. }))
            .count()
    }
    fn close(&mut self, key: FlowKey) {
        if let Some(flow) = self.flows.get_mut(&key) {
            flow.state = State::Closed {
                reusable_after: Instant::now() + REUSE_DELAY,
            };
        }
    }
}

struct DialResult {
    key: FlowKey,
    id: u64,
    generation: u64,
    result: io::Result<ByteStream>,
}
struct RelayResult {
    key: FlowKey,
    id: u64,
    result: io::Result<(u64, u64)>,
}

async fn run(
    mut input: mpsc::Receiver<Vec<u8>>,
    mut allowed: watch::Receiver<Policy>,
    mut stop: watch::Receiver<bool>,
    events: mpsc::Sender<Event>,
    connector: Arc<dyn Connector>,
    vpn: Ipv4Net,
    mtu: usize,
) -> io::Result<()> {
    let mut stack = match TcpStack::new(mtu, MAX_DIRECT) {
        Ok(stack) => stack,
        Err(error) => {
            connector.shutdown().await?;
            return Err(error);
        }
    };
    let mut state = Decisions::new();
    let mut dials = JoinSet::new();
    let mut relays = JoinSet::new();
    let mut aborts = JoinSet::new();
    let mut allowed_open = true;
    let mut traffic_tx = 0u64;
    let mut ticker = tokio::time::interval(Duration::from_millis(250));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let result = loop {
        tokio::select! {
            changed = stop.changed() => {
                if changed.is_err() || *stop.borrow() { break Ok(()); }
            }
            changed = allowed.changed(), if allowed_open => {
                if changed.is_err() { allowed_open = false; continue; }
                let networks = parse_allowed(&allowed.borrow_and_update().routes);
                let keys: Vec<_> = state.flows.keys().copied().collect();
                for key in keys {
                    let flow = state.flows.get_mut(&key).expect("collected key");
                    match &flow.state {
                        State::Pending { syn, abort } => {
                            abort.abort();
                            // Every config generation cancels pending physical dials.
                            // An unchanged authorization may safely begin on VPN.
                            if eligible(key, &networks, vpn) {
                                let _ = events.send(Event::ToVpn(syn.clone())).await;
                                flow.state = State::Vpn;
                            } else { state.close(key); }
                        }
                        State::Direct { abort } if !eligible(key, &networks, vpn) => {
                            abort.abort();
                            let handle = stack.handle.clone();
                            aborts.spawn(async move { handle.abort(key).await });
                            state.close(key);
                        }
                        _ => {}
                    }
                }
            }
            packet = input.recv() => {
                let Some(packet) = packet else { break Ok(()); };
                let Some(tcp) = Packet::parse(&packet) else {
                    // Fragmented/unsupported packets cannot be moved out of a
                    // committed stream. Drop ambiguous fragments until TCP retries
                    // without fragmentation; otherwise preserve the original VPN path.
                    let pair = address_pair(&packet);
                    if let Some(pair) = pair {
                        if state.fragmented_pairs.len() < MAX_HISTORY { state.fragmented_pairs.insert(pair); }
                        else { state.saturated = true; }
                    }
                    let committed = state.flows.iter().any(|(key, flow)|
                        pair == Some((*key.client.ip(), *key.target.ip()))
                        && !matches!(flow.state, State::Vpn));
                    if !committed { let _ = events.send(Event::ToVpn(packet)).await; }
                    continue;
                };
                if let Some(flow) = state.flows.get_mut(&tcp.key) {
                    match &flow.state {
                        State::Pending { abort, .. } => {
                            if tcp.flags & RST != 0 { abort.abort(); state.close(tcp.key); }
                            // Includes duplicate SYNs: no second dial and no premature ACK/data.
                            continue;
                        }
                        State::Direct { .. } => {
                            if stack.handle.try_send(packet).is_ok() { traffic_tx += tcp.len as u64; }
                            continue;
                        }
                        State::Closed { reusable_after } => {
                            if !tcp.initial() || tcp.isn == flow.isn || Instant::now() < *reusable_after { continue; }
                            // A genuinely new sequence space may reuse a closed tuple.
                            flow.state = State::Vpn;
                        }
                        State::Vpn if !tcp.initial() || tcp.isn == flow.isn => {
                            let _ = events.send(Event::ToVpn(packet)).await;
                            continue;
                        }
                        State::Vpn => {}
                    }
                }
                if !tcp.initial() {
                    state.pin_vpn(tcp.key);
                    let _ = events.send(Event::ToVpn(packet)).await;
                    continue;
                }
                let fresh = state.fresh(tcp.key, tcp.isn);
                let networks = parse_allowed(&allowed.borrow().routes);
                let can_dial = fresh && tcp.ordinary_syn(&packet) && eligible(tcp.key, &networks, vpn)
                    && !state.vpn_only.contains(&tcp.key)
                    && !state.fragmented_pairs.contains(&(*tcp.key.client.ip(), *tcp.key.target.ip()))
                    && valid_checksums(&packet) && state.pending() < MAX_PENDING
                    && state.active() < MAX_DIRECT && state.flows.len() < MAX_HISTORY;
                if !can_dial {
                    if let Some(flow) = state.flows.get_mut(&tcp.key) { flow.isn = tcp.isn; flow.state = State::Vpn; }
                    let _ = events.send(Event::ToVpn(packet)).await;
                    continue;
                }
                state.next_id += 1;
                let id = state.next_id;
                let key = tcp.key;
                let connector = connector.clone();
                let generation = allowed.borrow().generation;
                let abort = dials.spawn(async move { DialResult { key, id, generation, result: connector.connect(key.target).await } });
                state.flows.insert(key, Flow { isn: tcp.isn, id, state: State::Pending { syn: packet, abort } });
            }
            done = dials.join_next(), if !dials.is_empty() => {
                let Some(Ok(DialResult { key, id, generation, result })) = done else { continue; };
                let Some(flow) = state.flows.get(&key) else { continue; };
                if flow.id != id || !matches!(flow.state, State::Pending { .. }) { continue; }
                let State::Pending { syn, .. } = &flow.state else { unreachable!() };
                let syn = syn.clone();
                if !eligible(key, &parse_allowed(&allowed.borrow().routes), vpn) { state.close(key); continue; }
                if generation != allowed.borrow().generation {
                    state.flows.get_mut(&key).unwrap().state = State::Vpn;
                    let _ = events.send(Event::ToVpn(syn)).await;
                    continue;
                }
                let upstream = match result {
                    Ok(upstream) => upstream,
                    Err(error) => {
                        tracing::debug!(stage = "lan_tcp_dial", target = %key.target, error = %crate::error::redact_sensitive(&error.to_string()), "内网 TCP 未连通，本次连接使用 VPN");
                        state.flows.get_mut(&key).unwrap().state = State::Vpn;
                        let _ = events.send(Event::ToVpn(syn)).await;
                        continue;
                    }
                };
                match stack.handle.open(key, syn.clone()).await {
                    Ok(mut local) => {
                        // Config may have changed while the stack accepted the SYN.
                        if generation != allowed.borrow().generation || !eligible(key, &parse_allowed(&allowed.borrow().routes), vpn) {
                            let handle = stack.handle.clone();
                            aborts.spawn(async move { handle.abort(key).await });
                            state.close(key);
                            continue;
                        }
                        let mut upstream = upstream;
                        let abort = relays.spawn(async move {
                            RelayResult { key, id, result: tokio::io::copy_bidirectional(&mut local, &mut upstream).await }
                        });
                        state.flows.get_mut(&key).unwrap().state = State::Direct { abort };
                        traffic_tx += syn.len() as u64;
                        tracing::debug!(stage = "lan_tcp_dial", target = %key.target, "新 TCP 连接已直连内网");
                    }
                    Err(error) => {
                        // open errors are pre-commit: no SYN-ACK or application data was emitted.
                        tracing::warn!(stage = "lan_tcp_stack", error = %error, "本地 TCP 栈未接受连接，本次使用 VPN");
                        state.flows.get_mut(&key).unwrap().state = State::Vpn;
                        let _ = events.send(Event::ToVpn(syn)).await;
                    }
                }
            }
            done = relays.join_next(), if !relays.is_empty() => {
                let Some(Ok(RelayResult { key, id, result })) = done else { continue; };
                if state.flows.get(&key).is_some_and(|f| f.id == id && matches!(f.state, State::Direct { .. })) && result.is_err() {
                    let handle = stack.handle.clone();
                    aborts.spawn(async move { handle.abort(key).await });
                    state.close(key);
                }
                // Successful EOF must let smoltcp drain bytes and complete FIN;
                // copy_bidirectional completing is not a reason to send a RST.
            }
            closed = stack.closed.recv() => {
                let Some(key) = closed else { break Err(io::Error::other("TCP stack lifecycle channel closed")); };
                if let Some(Flow { state: State::Direct { abort }, .. }) = state.flows.get(&key) { abort.abort(); }
                state.close(key);
            }
            packet = stack.outbound.recv() => {
                let Some(packet) = packet else { break Err(io::Error::other("TCP stack output closed")); };
                let _ = events.send(Event::ToTun(packet)).await;
            }
            _ = aborts.join_next(), if !aborts.is_empty() => {}
            _ = ticker.tick() => {
                if traffic_tx > 0 {
                    let _ = events.send(Event::Traffic { tx: std::mem::take(&mut traffic_tx), rx: 0 }).await;
                }
            }
        }
    };
    dials.abort_all();
    relays.abort_all();
    while dials.join_next().await.is_some() {}
    while relays.join_next().await.is_some() {}
    stack.shutdown().await;
    aborts.abort_all();
    while aborts.join_next().await.is_some() {}
    connector.shutdown().await?;
    result
}

fn is_ipv4_tcp(packet: &[u8]) -> bool {
    packet.len() >= 20 && packet[0] >> 4 == 4 && packet[9] == 6
}
fn address_pair(packet: &[u8]) -> Option<(Ipv4Addr, Ipv4Addr)> {
    is_ipv4_tcp(packet).then(|| {
        (
            Ipv4Addr::new(packet[12], packet[13], packet[14], packet[15]),
            Ipv4Addr::new(packet[16], packet[17], packet[18], packet[19]),
        )
    })
}
fn parse_allowed(routes: &[String]) -> Vec<Ipv4Net> {
    routes
        .iter()
        .filter_map(|r| r.parse::<Ipv4Net>().ok())
        .filter(|r| r.prefix_len() != 0)
        .collect()
}
fn eligible(key: FlowKey, allowed: &[Ipv4Net], vpn: Ipv4Net) -> bool {
    let ip = key.target.ip();
    vpn.contains(key.client.ip())
        && key.client.port() != 0
        && key.target.port() != 0
        && !ip.is_unspecified()
        && !ip.is_loopback()
        && !ip.is_multicast()
        && !ip.is_broadcast()
        && !ip.is_link_local()
        && !vpn.contains(ip)
        && allowed.iter().any(|net| {
            net.contains(ip)
                && (net.prefix_len() >= 31 || (*ip != net.network() && *ip != net.broadcast()))
        })
}

struct Packet {
    key: FlowKey,
    isn: u32,
    flags: u8,
    len: usize,
    ip_len: usize,
    tcp_len: usize,
}
impl Packet {
    fn parse(packet: &[u8]) -> Option<Self> {
        let (src, dst) = address_pair(packet)?;
        let ip_len = usize::from(packet[0] & 15) * 4;
        let len = usize::from(u16::from_be_bytes([packet[2], packet[3]]));
        if ip_len < 20
            || len > packet.len()
            || len < ip_len + 20
            || u16::from_be_bytes([packet[6], packet[7]]) & !0x4000 != 0
        {
            return None;
        }
        let tcp = &packet[ip_len..len];
        let tcp_len = usize::from(tcp[12] >> 4) * 4;
        if tcp_len < 20 || tcp_len > tcp.len() {
            return None;
        }
        Some(Self {
            key: FlowKey {
                client: SocketAddrV4::new(src, u16::from_be_bytes([tcp[0], tcp[1]])),
                target: SocketAddrV4::new(dst, u16::from_be_bytes([tcp[2], tcp[3]])),
            },
            isn: u32::from_be_bytes(tcp[4..8].try_into().ok()?),
            flags: tcp[13],
            len,
            ip_len,
            tcp_len,
        })
    }
    fn initial(&self) -> bool {
        self.flags & (SYN | ACK) == SYN
    }
    fn ordinary_syn(&self, packet: &[u8]) -> bool {
        if self.ip_len != 20
            || self.len != self.ip_len + self.tcp_len
            || self.flags & !(SYN | 0xc0) != 0
        {
            return false;
        }
        let mut options = &packet[self.ip_len + 20..self.len];
        while !options.is_empty() {
            match options[0] {
                0 => return true,
                1 => options = &options[1..],
                kind => {
                    if options.len() < 2 {
                        return false;
                    }
                    let len = options[1] as usize;
                    if !matches!((kind, len), (2, 4) | (3, 3) | (4, 2) | (8, 10))
                        || len > options.len()
                    {
                        return false;
                    }
                    options = &options[len..];
                }
            }
        }
        true
    }
}

fn valid_checksums(packet: &[u8]) -> bool {
    use smoltcp::wire::{IpAddress, Ipv4Packet, TcpPacket};
    let Ok(ip) = Ipv4Packet::new_checked(packet) else {
        return false;
    };
    let Ok(tcp) = TcpPacket::new_checked(ip.payload()) else {
        return false;
    };
    ip.verify_checksum()
        && tcp.verify_checksum(
            &IpAddress::Ipv4(ip.src_addr()),
            &IpAddress::Ipv4(ip.dst_addr()),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt, DuplexStream},
        sync::Semaphore,
    };

    struct FakeConnector {
        calls: Mutex<Vec<SocketAddrV4>>,
        permits: Semaphore,
        peers: Mutex<Vec<DuplexStream>>,
        stopped: AtomicBool,
    }
    impl FakeConnector {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                calls: Mutex::new(vec![]),
                permits: Semaphore::new(0),
                peers: Mutex::new(vec![]),
                stopped: AtomicBool::new(false),
            })
        }
        async fn calls(&self, expected: usize) {
            tokio::time::timeout(Duration::from_secs(2), async {
                while self.calls.lock().unwrap().len() < expected {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("dial started");
        }
    }
    #[async_trait]
    impl Connector for FakeConnector {
        async fn connect(&self, target: SocketAddrV4) -> io::Result<ByteStream> {
            self.calls.lock().unwrap().push(target);
            self.permits.acquire().await.unwrap().forget();
            if target.port() != 8443 {
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionRefused,
                    "test physical path unavailable",
                ));
            }
            let (local, peer) = tokio::io::duplex(64 * 1024);
            self.peers.lock().unwrap().push(peer);
            Ok(Box::new(local))
        }
        async fn shutdown(&self) -> io::Result<()> {
            self.stopped.store(true, Ordering::SeqCst);
            Ok(())
        }
    }
    fn vpn() -> Ipv4Net {
        "100.96.0.0/24".parse().unwrap()
    }
    fn allowed() -> Vec<String> {
        vec!["192.168.188.0/24".into(), "192.168.186.0/24".into()]
    }
    fn router(fake: &Arc<FakeConnector>) -> Router {
        Router::spawn(allowed(), vpn(), 1420, Ok(fake.clone()))
    }
    fn key(port: u16, target: u16) -> FlowKey {
        FlowKey {
            client: format!("100.96.0.2:{port}").parse().unwrap(),
            target: format!("192.168.188.111:{target}").parse().unwrap(),
        }
    }
    fn packet(key: FlowKey, seq: u32, ack: u32, flags: u8, payload: &[u8]) -> Vec<u8> {
        use smoltcp::wire::{IpAddress, Ipv4Packet, TcpPacket};
        let len = 40 + payload.len();
        let mut bytes = vec![0; len];
        bytes[0] = 0x45;
        bytes[2..4].copy_from_slice(&(len as u16).to_be_bytes());
        bytes[6..8].copy_from_slice(&0x4000u16.to_be_bytes());
        bytes[8] = 64;
        bytes[9] = 6;
        bytes[12..16].copy_from_slice(&key.client.ip().octets());
        bytes[16..20].copy_from_slice(&key.target.ip().octets());
        bytes[20..22].copy_from_slice(&key.client.port().to_be_bytes());
        bytes[22..24].copy_from_slice(&key.target.port().to_be_bytes());
        bytes[24..28].copy_from_slice(&seq.to_be_bytes());
        bytes[28..32].copy_from_slice(&ack.to_be_bytes());
        bytes[32] = 0x50;
        bytes[33] = flags;
        bytes[34..36].copy_from_slice(&65535u16.to_be_bytes());
        bytes[40..].copy_from_slice(payload);
        Ipv4Packet::new_unchecked(&mut bytes[..]).fill_checksum();
        TcpPacket::new_unchecked(&mut bytes[20..]).fill_checksum(
            &IpAddress::Ipv4(*key.client.ip()),
            &IpAddress::Ipv4(*key.target.ip()),
        );
        bytes
    }
    async fn next_packet(router: &mut Router) -> Event {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match router.recv().await.expect("router running") {
                    Event::Traffic { .. } => {}
                    event => return event,
                }
            }
        })
        .await
        .expect("packet emitted")
    }
    async fn quiet(router: &mut Router) {
        assert!(
            tokio::time::timeout(Duration::from_millis(30), next_packet(router))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn failed_dial_releases_original_syn_once_and_retransmits_only_on_vpn() {
        let fake = FakeConnector::new();
        let mut router = router(&fake);
        let syn = packet(key(41000, 22), 1000, 0, SYN, &[]);
        assert!(router.try_send(syn.clone()));
        fake.calls(1).await;
        for _ in 0..5 {
            router.try_send(syn.clone());
        }
        quiet(&mut router).await;
        assert_eq!(fake.calls.lock().unwrap().len(), 1);
        fake.permits.add_permits(1);
        match next_packet(&mut router).await {
            Event::ToVpn(p) => assert_eq!(p, syn),
            _ => panic!("must fall back"),
        }
        router.try_send(syn.clone());
        match next_packet(&mut router).await {
            Event::ToVpn(p) => assert_eq!(p, syn),
            _ => panic!("retransmission switched path"),
        }
        assert_eq!(fake.calls.lock().unwrap().len(), 1);
        router.shutdown().await.unwrap();
        assert!(fake.stopped.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn first_direct_connection_keeps_one_socket_and_relays_opaque_bytes() {
        let fake = FakeConnector::new();
        let mut router = router(&fake);
        let key = key(41001, 8443);
        let syn = packet(key, 1000, 0, SYN, &[]);
        router.try_send(syn.clone());
        fake.calls(1).await;
        fake.permits.add_permits(1);
        let syn_ack = match next_packet(&mut router).await {
            Event::ToTun(p) => p,
            _ => panic!("reachable first connection went to VPN"),
        };
        let response = Packet::parse(&syn_ack).unwrap();
        assert_eq!(response.flags & (SYN | ACK), SYN | ACK);
        let remote_seq = response.isn.wrapping_add(1);
        router.try_send(packet(key, 1001, remote_seq, ACK, &[]));
        let mut peer = fake.peers.lock().unwrap().pop().unwrap();
        let request = b"opaque TLS or SSH bytes";
        router.try_send(packet(key, 1001, remote_seq, ACK | 8, request));
        let mut received = vec![0; request.len()];
        tokio::time::timeout(Duration::from_secs(2), peer.read_exact(&mut received))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&received, request);
        peer.write_all(b"reply").await.unwrap();
        loop {
            match next_packet(&mut router).await {
                Event::ToTun(p) => {
                    let parsed = Packet::parse(&p).unwrap();
                    if parsed.len > parsed.ip_len + parsed.tcp_len {
                        assert_eq!(&p[parsed.ip_len + parsed.tcp_len..parsed.len], b"reply");
                        break;
                    }
                }
                _ => panic!("committed data sent through VPN"),
            }
        }
        router.try_send(syn); // even a stale SYN cannot create a second upstream
        assert_eq!(fake.calls.lock().unwrap().len(), 1);
        router.shutdown().await.unwrap();
        assert!(fake.stopped.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn same_ip_different_connections_can_use_direct_and_vpn_concurrently() {
        let fake = FakeConnector::new();
        let mut router = router(&fake);
        let direct = packet(key(42000, 8443), 1, 0, SYN, &[]);
        let remote = packet(key(42001, 22), 1, 0, SYN, &[]);
        router.try_send(direct);
        router.try_send(remote.clone());
        fake.calls(2).await;
        fake.permits.add_permits(2);
        let mut paths = (0, 0);
        for _ in 0..2 {
            match next_packet(&mut router).await {
                Event::ToTun(_) => paths.0 += 1,
                Event::ToVpn(p) => {
                    assert_eq!(p, remote);
                    paths.1 += 1;
                }
                _ => unreachable!(),
            }
        }
        assert_eq!(paths, (1, 1));
        router.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn config_generation_cancels_pending_and_revocation_closes_direct() {
        let fake = FakeConnector::new();
        let mut router = router(&fake);
        let pending = packet(key(43000, 8443), 1, 0, SYN, &[]);
        router.try_send(pending.clone());
        fake.calls(1).await;
        router.set_allowed(vec![]);
        fake.permits.add_permits(1);
        quiet(&mut router).await;
        router.try_send(pending);
        quiet(&mut router).await;
        router.set_allowed(allowed());
        let direct = key(43001, 8443);
        router.try_send(packet(direct, 2, 0, SYN, &[]));
        fake.calls(2).await;
        fake.permits.add_permits(1);
        assert!(matches!(next_packet(&mut router).await, Event::ToTun(_)));
        router.set_allowed(vec![]);
        loop {
            match next_packet(&mut router).await {
                Event::ToTun(p) if Packet::parse(&p).unwrap().flags & RST != 0 => break,
                Event::ToTun(_) => {}
                _ => panic!("revoked direct flow fell back to VPN"),
            }
        }
        router.try_send(packet(direct, 3, 100, ACK | 8, b"never replay"));
        quiet(&mut router).await;
        router.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn unsupported_syn_and_fragment_retransmissions_never_start_a_dial() {
        let fake = FakeConnector::new();
        let mut router = router(&fake);
        let tfo = packet(key(44000, 8443), 1, 0, SYN, b"early data");
        router.try_send(tfo.clone());
        assert!(matches!(next_packet(&mut router).await, Event::ToVpn(p) if p == tfo));
        let mut fragment = packet(key(44001, 8443), 1, 0, SYN, &[]);
        fragment[6..8].copy_from_slice(&0x2000u16.to_be_bytes());
        router.try_send(fragment.clone());
        assert!(matches!(next_packet(&mut router).await, Event::ToVpn(p) if p == fragment));
        let retry = packet(key(44001, 8443), 1, 0, SYN, &[]);
        router.try_send(retry.clone());
        assert!(matches!(next_packet(&mut router).await, Event::ToVpn(p) if p == retry));
        assert!(fake.calls.lock().unwrap().is_empty());
        router.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn saturated_input_and_output_can_still_shutdown_and_join() {
        let fake = FakeConnector::new();
        let router = router(&fake);
        for port in 1..=1000 {
            router.try_send(packet(key(port, 22), 1, 1, ACK, &[]));
        }
        tokio::task::yield_now().await;
        tokio::time::timeout(Duration::from_secs(2), router.shutdown())
            .await
            .unwrap()
            .unwrap();
        assert!(fake.stopped.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn unavailable_physical_backend_keeps_vpn_running() {
        let mut router = Router::spawn(
            allowed(),
            vpn(),
            1420,
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "policy unavailable",
            )),
        );
        let syn = packet(key(44002, 8443), 1, 0, SYN, &[]);
        router.try_send(syn.clone());
        assert!(matches!(next_packet(&mut router).await, Event::ToVpn(p) if p == syn));
        router.shutdown().await.unwrap();
    }

    #[test]
    fn configured_subnets_are_dynamic_and_source_and_special_addresses_are_excluded() {
        let networks = parse_allowed(&allowed());
        let mut target = key(45000, 8443);
        assert!(eligible(target, &networks, vpn()));
        target.target.set_ip("192.168.186.10".parse().unwrap());
        assert!(eligible(target, &networks, vpn()));
        target.target.set_ip("192.168.187.10".parse().unwrap());
        assert!(!eligible(target, &networks, vpn()));
        for ip in [
            "192.168.186.0",
            "192.168.186.255",
            "100.96.0.3",
            "127.0.0.1",
            "224.0.0.1",
        ] {
            target.target.set_ip(ip.parse().unwrap());
            assert!(!eligible(target, &networks, vpn()));
        }
        target.target.set_ip("192.168.186.10".parse().unwrap());
        target.client.set_ip("192.168.187.2".parse().unwrap());
        assert!(
            !eligible(target, &networks, vpn()),
            "physical socket cannot recursively enter proxy"
        );
        assert!(parse_allowed(&["0.0.0.0/0".into(), "::/0".into(), "invalid".into()]).is_empty());
    }

    #[test]
    fn history_limits_preserve_existing_decisions_and_pin_new_connections_to_vpn() {
        let mut decisions = Decisions::new();
        let key = key(45001, 8443);
        assert!(decisions.fresh(key, 1));
        assert!(!decisions.fresh(key, 1));
        for isn in 2..=MAX_HISTORY as u32 {
            assert!(decisions.fresh(key, isn));
        }
        assert!(!decisions.fresh(key, MAX_HISTORY as u32 + 1));
        assert_eq!(decisions.attempted.len(), MAX_HISTORY);
        assert!(decisions.saturated);
    }
}
