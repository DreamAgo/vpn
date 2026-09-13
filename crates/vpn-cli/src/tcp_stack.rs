//! Bounded IPv4 TCP packet/stream adapter for connections selected for LAN delivery.
//!
//! The dispatcher must establish the physical upstream before calling `open`.
//! TCP sequence numbers, retransmissions, congestion control, windows, and FIN/RST
//! handling belong to smoltcp. This module only connects its socket buffers to
//! Tokio byte streams and owns their lifetime; it does not implement TCP.

use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    io,
    net::SocketAddrV4,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use smoltcp::{
    iface::{Config, Interface, PollIngressSingleResult, SocketHandle, SocketSet},
    phy::{ChecksumCapabilities, Device, DeviceCapabilities, Medium, RxToken, TxToken},
    socket::tcp,
    time::Instant,
    wire::{
        HardwareAddress, IpAddress, IpCidr, IpProtocol, Ipv4Address, Ipv4Packet, TcpPacket, TcpRepr,
    },
};
use tokio::{
    io::{AsyncRead, AsyncWrite, DuplexStream, ReadBuf},
    sync::{mpsc, oneshot},
    task::JoinHandle,
    time::{Interval, MissedTickBehavior},
};

const PACKET_CAPACITY: usize = 256;
const WINDOW_BYTES: usize = 64 * 1024;
const IO_CHUNK: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct FlowKey {
    pub client: SocketAddrV4,
    pub target: SocketAddrV4,
}

enum Command {
    Open {
        key: FlowKey,
        syn: Vec<u8>,
        reply: oneshot::Sender<io::Result<DuplexStream>>,
    },
    Abort(FlowKey, oneshot::Sender<io::Result<()>>),
}

#[derive(Clone)]
pub(crate) struct StackHandle {
    commands: mpsc::Sender<Command>,
    packets: mpsc::Sender<Vec<u8>>,
    mtu: usize,
}

impl StackHandle {
    /// Commit an already connected physical path. No packet is emitted before
    /// this call. A failed call leaves the original SYN available to the caller
    /// for VPN fallback (the caller should retain its original copy).
    pub async fn open(&self, key: FlowKey, syn: Vec<u8>) -> io::Result<DuplexStream> {
        if syn.len() > self.mtu || !valid_syn(&syn, key) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid TCP SYN",
            ));
        }
        let (reply, result) = oneshot::channel();
        self.commands
            .send(Command::Open { key, syn, reply })
            .await
            .map_err(|_| stack_closed())?;
        result.await.map_err(|_| stack_closed())?
    }

    /// Drop on queue saturation and let TCP retransmit. A packet already
    /// committed to this stack must never be rerouted into WireGuard.
    pub fn try_send(&self, packet: Vec<u8>) -> Result<(), mpsc::error::TrySendError<Vec<u8>>> {
        if packet.len() > self.mtu {
            return Err(mpsc::error::TrySendError::Full(packet));
        }
        self.packets.try_send(packet)
    }

    /// Tear down both halves after relay failure, cancellation, or revocation.
    pub async fn abort(&self, key: FlowKey) -> io::Result<()> {
        let (reply, result) = oneshot::channel();
        self.commands
            .send(Command::Abort(key, reply))
            .await
            .map_err(|_| stack_closed())?;
        result.await.map_err(|_| stack_closed())?
    }
}

fn stack_closed() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "local TCP stack stopped")
}

pub(crate) struct TcpStack {
    pub handle: StackHandle,
    pub outbound: mpsc::Receiver<Vec<u8>>,
    /// Natural closure, including peer RST. Explicit abort uses its reply.
    pub closed: mpsc::Receiver<FlowKey>,
    runner: Option<JoinHandle<()>>,
}

impl TcpStack {
    pub fn new(mtu: usize, max_flows: usize) -> io::Result<Self> {
        Self::with_output_capacity(mtu, max_flows, PACKET_CAPACITY)
    }

    fn with_output_capacity(
        mtu: usize,
        max_flows: usize,
        output_capacity: usize,
    ) -> io::Result<Self> {
        if !(576..=u16::MAX as usize).contains(&mtu) || !(1..=1024).contains(&max_flows) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid TCP stack limits",
            ));
        }
        let (command_tx, commands) = mpsc::channel(max_flows);
        let (packet_tx, packets) = mpsc::channel(PACKET_CAPACITY);
        let (output, outbound) = mpsc::channel(output_capacity);
        let (closed_tx, closed) = mpsc::channel(max_flows);
        let mut device = PacketDevice {
            incoming: VecDeque::new(),
            output,
            mtu,
        };
        let mut config = Config::new(HardwareAddress::Ip);
        config.random_seed = rand::random();
        let mut interface = Interface::new(config, &mut device, Instant::now());
        // Any-IP lets one interface terminate each authorized original target.
        // Ingress is separately restricted to explicitly registered tuples.
        interface.update_ip_addrs(|addresses| {
            addresses
                .push(IpCidr::new(
                    IpAddress::Ipv4(Ipv4Address::new(0, 0, 0, 1)),
                    0,
                ))
                .unwrap();
        });
        interface
            .routes_mut()
            .add_default_ipv4_route(Ipv4Address::new(0, 0, 0, 1))
            .map_err(io::Error::other)?;
        interface.set_any_ip(true);
        let mut timer = tokio::time::interval(Duration::from_millis(5));
        timer.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let runner = tokio::spawn(StackRunner {
            interface,
            device,
            sockets: SocketSet::new(vec![]),
            flows: HashMap::new(),
            commands,
            packets,
            max_flows,
            timer,
            closed: closed_tx,
        });
        Ok(Self {
            handle: StackHandle {
                commands: command_tx,
                packets: packet_tx,
                mtu,
            },
            outbound,
            closed,
            runner: Some(runner),
        })
    }

    pub async fn shutdown(mut self) {
        if let Some(runner) = self.runner.take() {
            runner.abort();
            let _ = runner.await;
        }
    }
}

impl Drop for TcpStack {
    fn drop(&mut self) {
        if let Some(runner) = &self.runner {
            runner.abort();
        }
    }
}

struct Flow {
    handle: SocketHandle,
    stream: Option<DuplexStream>,
    read_eof: bool,
    write_eof: bool,
    aborting: bool,
    abort_reply: Option<oneshot::Sender<io::Result<()>>>,
    created: std::time::Instant,
}

struct StackRunner {
    interface: Interface,
    device: PacketDevice,
    sockets: SocketSet<'static>,
    flows: HashMap<FlowKey, Flow>,
    commands: mpsc::Receiver<Command>,
    packets: mpsc::Receiver<Vec<u8>>,
    max_flows: usize,
    timer: Interval,
    closed: mpsc::Sender<FlowKey>,
}

impl StackRunner {
    fn poll_packets(&mut self) {
        while self.interface.poll_ingress_single(
            Instant::now(),
            &mut self.device,
            &mut self.sockets,
        ) != PollIngressSingleResult::None
        {
            // A passive smoltcp socket returns to Listen if it receives a
            // valid RST while SynReceived. Our streams are per connection,
            // not reusable listeners: retire it before the next packet could
            // bind it to another client's SYN for the same destination.
            for flow in self.flows.values_mut() {
                let socket = self.sockets.get_mut::<tcp::Socket>(flow.handle);
                if socket.state() == tcp::State::Listen {
                    socket.close();
                    flow.aborting = true;
                    flow.stream.take();
                }
            }
        }
        self.interface
            .poll_egress(Instant::now(), &mut self.device, &mut self.sockets);
    }

    fn open(&mut self, key: FlowKey, syn: Vec<u8>) -> io::Result<DuplexStream> {
        if self.flows.contains_key(&key) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "TCP tuple already registered",
            ));
        }
        self.poll_packets();
        if self.flows.len() >= self.max_flows
            || !self.device.incoming.is_empty()
            || self.device.output.capacity() == 0
        {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "local TCP stack is full",
            ));
        }
        let mut socket = tcp::Socket::new(
            tcp::SocketBuffer::new(vec![0; WINDOW_BYTES]),
            tcp::SocketBuffer::new(vec![0; WINDOW_BYTES]),
        );
        socket.set_ack_delay(None);
        socket.set_keep_alive(Some(smoltcp::time::Duration::from_secs(30)));
        socket.set_timeout(Some(smoltcp::time::Duration::from_secs(7200)));
        socket.listen(key.target).map_err(io::Error::other)?;
        let handle = self.sockets.add(socket);
        let (application, stream) = tokio::io::duplex(WINDOW_BYTES);
        self.flows.insert(
            key,
            Flow {
                handle,
                stream: Some(stream),
                read_eof: false,
                write_eof: false,
                aborting: false,
                abort_reply: None,
                created: std::time::Instant::now(),
            },
        );
        self.device.incoming.push_back(syn);
        // Process this SYN immediately, so a subsequent listener on the same
        // target cannot consume its peer's first packet.
        self.poll_packets();
        let socket = self.sockets.get::<tcp::Socket>(handle);
        if socket.state() != tcp::State::SynReceived
            || socket.local_endpoint() != Some(key.target.into())
            || socket.remote_endpoint() != Some(key.client.into())
        {
            // Parsing a SYN does not guarantee smoltcp accepts it. Never
            // retain an unbound listener: it could consume another client's
            // SYN to the same destination. Remove it without abort/RST so a
            // rejected, uncommitted SYN can still follow the VPN path.
            self.flows.remove(&key);
            self.sockets.remove(handle);
            self.device
                .incoming
                .retain(|packet| packet_key(packet) != Some(key));
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "TCP stack did not accept the initial SYN",
            ));
        }
        Ok(application)
    }

    fn abort(&mut self, key: FlowKey, reply: Option<oneshot::Sender<io::Result<()>>>) {
        if let Some(flow) = self.flows.get_mut(&key) {
            if flow
                .abort_reply
                .as_ref()
                .is_some_and(|reply| !reply.is_closed())
            {
                if let Some(reply) = reply {
                    let _ = reply.send(Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "TCP abort pending",
                    )));
                }
                return;
            }
            self.sockets.get_mut::<tcp::Socket>(flow.handle).abort();
            flow.aborting = true;
            flow.abort_reply = reply;
            flow.stream.take();
            self.device
                .incoming
                .retain(|packet| packet_key(packet) != Some(key));
        } else if let Some(reply) = reply {
            let _ = reply.send(Ok(()));
        }
    }

    fn pump_streams(&mut self, cx: &mut Context<'_>) -> bool {
        let mut changed = false;
        let mut scratch = [0u8; IO_CHUNK];
        for flow in self.flows.values_mut() {
            let socket = self.sockets.get_mut::<tcp::Socket>(flow.handle);
            if flow.aborting {
                continue;
            }
            if matches!(socket.state(), tcp::State::Listen | tcp::State::SynReceived)
                && flow.created.elapsed() >= Duration::from_secs(30)
            {
                socket.abort();
                flow.aborting = true;
                flow.stream.take();
                changed = true;
                continue;
            }
            let Some(stream) = flow.stream.as_mut() else {
                continue;
            };
            // Keep unread bytes in smoltcp if the duplex buffer is full. This
            // advertises backpressure through the real TCP receive window.
            if socket.can_recv() && !flow.write_eof {
                let _ = socket.recv(|bytes| match Pin::new(&mut *stream).poll_write(cx, bytes) {
                    Poll::Ready(Ok(n)) => {
                        changed |= n > 0;
                        (n, ())
                    }
                    Poll::Ready(Err(_)) => {
                        flow.aborting = true;
                        (0, ())
                    }
                    Poll::Pending => (0, ()),
                });
            }
            // A peer FIN ends only this direction. The physical upstream may
            // still send its final response through the other duplex half.
            if !socket.may_recv()
                && !socket.can_recv()
                && !flow.write_eof
                && !matches!(socket.state(), tcp::State::Listen | tcp::State::SynReceived)
                && Pin::new(&mut *stream).poll_shutdown(cx).is_ready()
            {
                flow.write_eof = true;
                changed = true;
            }
            if socket.can_send() && !flow.read_eof {
                let available = (socket.send_capacity() - socket.send_queue()).min(scratch.len());
                if available > 0 {
                    let mut buffer = ReadBuf::new(&mut scratch[..available]);
                    match Pin::new(&mut *stream).poll_read(cx, &mut buffer) {
                        Poll::Ready(Ok(())) if buffer.filled().is_empty() => {
                            flow.read_eof = true;
                            socket.close();
                            changed = true;
                        }
                        Poll::Ready(Ok(())) => {
                            let sent = socket.send_slice(buffer.filled()).unwrap_or(0);
                            debug_assert_eq!(sent, buffer.filled().len());
                            changed |= sent > 0;
                        }
                        Poll::Ready(Err(_)) => flow.aborting = true,
                        Poll::Pending => {}
                    }
                }
            }
            if flow.aborting {
                socket.abort();
                changed = true;
            }
        }
        changed
    }
}

impl Future for StackRunner {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = self.get_mut();
        if this.device.output.is_closed() {
            return Poll::Ready(());
        }
        // Register the timer even when the caller's byte stream currently has
        // no IO; TCP retransmission and keepalive timers must continue to run.
        // Poll again after consuming a ready tick: reset() alone does not
        // register a new wakeup for an otherwise idle, backpressured stack.
        while this.timer.poll_tick(cx).is_ready() {}
        let mut progressed = false;
        for _ in 0..32 {
            match this.commands.poll_recv(cx) {
                Poll::Ready(Some(Command::Open { key, syn, reply })) => {
                    if !reply.is_closed() {
                        let result = this.open(key, syn);
                        if let Err(Ok(_)) = reply.send(result) {
                            this.abort(key, None);
                        }
                    }
                    progressed = true;
                }
                Poll::Ready(Some(Command::Abort(key, reply))) => {
                    this.abort(key, Some(reply));
                    progressed = true;
                }
                Poll::Ready(None) => return Poll::Ready(()),
                Poll::Pending => break,
            }
        }
        for _ in 0..64 {
            if this.device.incoming.len() >= PACKET_CAPACITY {
                break;
            }
            match this.packets.poll_recv(cx) {
                Poll::Ready(Some(packet)) => {
                    if packet_key(&packet)
                        .is_some_and(|key| this.flows.get(&key).is_some_and(|flow| !flow.aborting))
                    {
                        this.device.incoming.push_back(packet);
                    }
                    progressed = true;
                }
                Poll::Ready(None) | Poll::Pending => break,
            }
        }
        this.poll_packets();
        progressed |= this.pump_streams(cx);
        this.poll_packets();
        // All flow buffers and the peer duplex half are released here. The
        // socket remains bounded by max_flows during normal TIME-WAIT.
        let closed: Vec<_> = this
            .flows
            .iter()
            .filter_map(|(key, flow)| {
                let socket = this.sockets.get::<tcp::Socket>(flow.handle);
                // A tuple is retained until an abort's RST is actually queued,
                // including when the bounded egress is temporarily full.
                (socket.state() == tcp::State::Closed && socket.remote_endpoint().is_none())
                    .then_some(*key)
            })
            .collect();
        for key in closed {
            if this.flows[&key].abort_reply.is_none() && this.closed.try_send(key).is_err() {
                // Keeping its flow slot also bounds undelivered completions.
                continue;
            }
            if let Some(flow) = this.flows.remove(&key) {
                this.sockets.remove(flow.handle);
                if let Some(reply) = flow.abort_reply {
                    let _ = reply.send(Ok(()));
                }
            }
        }
        if progressed {
            cx.waker().wake_by_ref();
        }
        Poll::Pending
    }
}

fn packet_key(packet: &[u8]) -> Option<FlowKey> {
    let ip = Ipv4Packet::new_checked(packet).ok()?;
    if ip.next_header() != IpProtocol::Tcp || ip.frag_offset() != 0 || ip.more_frags() {
        return None;
    }
    let tcp = TcpPacket::new_checked(ip.payload()).ok()?;
    Some(FlowKey {
        client: SocketAddrV4::new(ip.src_addr(), tcp.src_port()),
        target: SocketAddrV4::new(ip.dst_addr(), tcp.dst_port()),
    })
}

fn valid_syn(packet: &[u8], key: FlowKey) -> bool {
    let Ok(ip) = Ipv4Packet::new_checked(packet) else {
        return false;
    };
    if !ip.verify_checksum() || packet_key(packet) != Some(key) {
        return false;
    }
    let Ok(tcp) = TcpPacket::new_checked(ip.payload()) else {
        return false;
    };
    tcp.syn()
        && !tcp.ack()
        && !tcp.rst()
        && !tcp.fin()
        && tcp.payload().is_empty()
        && TcpRepr::parse(
            &tcp,
            &IpAddress::Ipv4(ip.src_addr()),
            &IpAddress::Ipv4(ip.dst_addr()),
            &ChecksumCapabilities::default(),
        )
        .is_ok_and(|repr| repr.max_seg_size != Some(0))
}

struct PacketDevice {
    incoming: VecDeque<Vec<u8>>,
    output: mpsc::Sender<Vec<u8>>,
    mtu: usize,
}

impl Device for PacketDevice {
    type RxToken<'a> = PacketRx;
    type TxToken<'a> = PacketTx<'a>;

    fn receive(&mut self, _: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let permit = self.output.try_reserve().ok()?;
        let packet = self.incoming.pop_front()?;
        Some((PacketRx(packet), PacketTx(permit)))
    }

    fn transmit(&mut self, _: Instant) -> Option<Self::TxToken<'_>> {
        self.output.try_reserve().ok().map(PacketTx)
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ip;
        caps.max_transmission_unit = self.mtu;
        caps
    }
}

struct PacketRx(Vec<u8>);
impl RxToken for PacketRx {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        f(&self.0)
    }
}
struct PacketTx<'a>(mpsc::Permit<'a, Vec<u8>>);
impl TxToken for PacketTx<'_> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let mut packet = vec![0; len];
        let result = f(&mut packet);
        self.0.send(packet);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    struct Client {
        key: FlowKey,
        interface: Interface,
        sockets: SocketSet<'static>,
        socket: SocketHandle,
        device: PacketDevice,
        outgoing: mpsc::Receiver<Vec<u8>>,
    }

    impl Client {
        fn new(port: u16) -> Self {
            let key = FlowKey {
                client: SocketAddrV4::new(Ipv4Address::new(100, 96, 0, 2), port),
                target: SocketAddrV4::new(Ipv4Address::new(192, 168, 188, 111), 8443),
            };
            let (output, outgoing) = mpsc::channel(PACKET_CAPACITY);
            let mut device = PacketDevice {
                incoming: VecDeque::new(),
                output,
                mtu: 1420,
            };
            let mut config = Config::new(HardwareAddress::Ip);
            config.random_seed = u64::from(port);
            let mut interface = Interface::new(config, &mut device, Instant::now());
            interface.update_ip_addrs(|addresses| {
                addresses
                    .push(IpCidr::new(IpAddress::Ipv4(*key.client.ip()), 0))
                    .unwrap();
            });
            let mut sockets = SocketSet::new(vec![]);
            let mut socket = tcp::Socket::new(
                tcp::SocketBuffer::new(vec![0; WINDOW_BYTES]),
                tcp::SocketBuffer::new(vec![0; WINDOW_BYTES]),
            );
            socket.set_ack_delay(None);
            socket
                .connect(interface.context(), key.target, key.client)
                .unwrap();
            let socket = sockets.add(socket);
            Self {
                key,
                interface,
                sockets,
                socket,
                device,
                outgoing,
            }
        }

        fn poll(&mut self) {
            self.interface
                .poll(Instant::now(), &mut self.device, &mut self.sockets);
        }

        fn syn(&mut self) -> Vec<u8> {
            self.poll();
            self.outgoing.try_recv().expect("client emits first SYN")
        }

        async fn establish(&mut self, stack: &mut TcpStack) {
            tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    while let Ok(packet) = stack.outbound.try_recv() {
                        self.device.incoming.push_back(packet);
                    }
                    self.poll();
                    while let Ok(packet) = self.outgoing.try_recv() {
                        stack.handle.try_send(packet).unwrap();
                    }
                    if self.sockets.get::<tcp::Socket>(self.socket).state()
                        == tcp::State::Established
                    {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            })
            .await
            .expect("client completes TCP handshake");
        }
    }

    // Two actual smoltcp TCP stacks exchange IP packets. No OS sockets, raw
    // sockets, route changes, TUN device, or fabricated TCP state machine.
    #[tokio::test]
    async fn real_tcp_handshake_large_echo_and_half_close() {
        let mut stack = TcpStack::new(1420, 4).unwrap();
        let mut client = Client::new(42000);
        let syn = client.syn();
        assert!(
            stack.outbound.try_recv().is_err(),
            "no response before commitment"
        );
        let mut stream = stack.handle.open(client.key, syn.clone()).await.unwrap();
        // SYN retransmissions stay with the existing socket. They must not
        // allocate a second wildcard listener or replace the byte stream.
        stack.handle.try_send(syn).unwrap();
        let reply = tokio::spawn(async move {
            let mut body = Vec::new();
            stream.read_to_end(&mut body).await.unwrap();
            stream.write_all(&body).await.unwrap();
            stream.shutdown().await.unwrap();
            body.len()
        });
        let payload: Vec<_> = (0..WINDOW_BYTES * 5).map(|i| (i % 251) as u8).collect();
        let mut sent = 0;
        let mut received = Vec::new();
        let mut sent_fin = false;
        let mut dropped_segment = false;
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                while let Ok(packet) = stack.outbound.try_recv() {
                    client.device.incoming.push_back(packet);
                }
                client.poll();
                let socket = client.sockets.get_mut::<tcp::Socket>(client.socket);
                if socket.can_send() && sent < payload.len() {
                    sent += socket.send_slice(&payload[sent..]).unwrap();
                }
                if sent == payload.len() && !sent_fin {
                    socket.close();
                    sent_fin = true;
                }
                if socket.can_recv() {
                    socket
                        .recv(|bytes| {
                            received.extend_from_slice(bytes);
                            (bytes.len(), ())
                        })
                        .unwrap();
                }
                let done = sent_fin && !socket.may_recv() && received.len() == payload.len();
                client.poll();
                while let Ok(packet) = client.outgoing.try_recv() {
                    let ip = Ipv4Packet::new_checked(&packet).unwrap();
                    let tcp = TcpPacket::new_checked(ip.payload()).unwrap();
                    // Lose one data segment, leaving later segments in flight.
                    // Recovery must come from TCP retransmission/reassembly.
                    if !dropped_segment && !tcp.payload().is_empty() {
                        dropped_segment = true;
                        continue;
                    }
                    stack.handle.try_send(packet).unwrap();
                }
                if done {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("TCP exchange and half-close finish");
        assert!(dropped_segment);
        assert_eq!(received, payload);
        assert_eq!(reply.await.unwrap(), payload.len());
        stack.shutdown().await;
    }

    #[tokio::test]
    async fn capacity_and_duplicate_open_do_not_ack_a_fallback_syn() {
        let mut stack = TcpStack::new(1420, 1).unwrap();
        let mut first = Client::new(42001);
        let first_syn = first.syn();
        let _stream = stack
            .handle
            .open(first.key, first_syn.clone())
            .await
            .unwrap();
        assert_eq!(
            stack
                .handle
                .open(first.key, first_syn)
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        let mut second = Client::new(42002);
        let second_syn = second.syn();
        assert_eq!(
            stack
                .handle
                .open(second.key, second_syn)
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::WouldBlock
        );
        while let Ok(packet) = stack.outbound.try_recv() {
            let ip = Ipv4Packet::new_checked(&packet).unwrap();
            let tcp = TcpPacket::new_checked(ip.payload()).unwrap();
            assert_eq!(tcp.dst_port(), first.key.client.port());
        }
        stack.shutdown().await;
    }

    #[tokio::test]
    async fn shutdown_closes_streams_and_both_input_channels() {
        let stack = TcpStack::new(1420, 1).unwrap();
        let handle = stack.handle.clone();
        let mut client = Client::new(42003);
        let mut stream = handle.open(client.key, client.syn()).await.unwrap();
        stack.shutdown().await;
        assert!(handle.packets.is_closed());
        assert!(handle.commands.is_closed());
        let mut byte = [0u8; 1];
        assert_eq!(stream.read(&mut byte).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn invalid_syn_has_no_socket_and_no_response() {
        let mut stack = TcpStack::new(1420, 1).unwrap();
        let mut client = Client::new(42004);
        let mut syn = client.syn();
        syn[10] ^= 1;
        assert_eq!(
            stack.handle.open(client.key, syn).await.unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert!(stack.outbound.try_recv().is_err());
        stack.shutdown().await;
    }

    #[tokio::test]
    async fn zero_mss_syn_cannot_leave_listener_to_steal_another_tuple() {
        let mut stack = TcpStack::new(1420, 1).unwrap();
        let mut rejected = Client::new(42012);
        let mut syn = rejected.syn();
        let ip = Ipv4Packet::new_checked(&syn).unwrap();
        let ip_header_len = usize::from(ip.header_len());
        let source = IpAddress::Ipv4(ip.src_addr());
        let destination = IpAddress::Ipv4(ip.dst_addr());
        let tcp = TcpPacket::new_checked(ip.payload()).unwrap();
        let mut repr = TcpRepr::parse(
            &tcp,
            &source,
            &destination,
            &ChecksumCapabilities::default(),
        )
        .unwrap();
        assert!(repr.max_seg_size.is_some());
        repr.max_seg_size = Some(0);
        let mut tcp_bytes = vec![0; repr.buffer_len()];
        repr.emit(
            &mut TcpPacket::new_unchecked(&mut tcp_bytes),
            &source,
            &destination,
            &ChecksumCapabilities::default(),
        );
        syn[ip_header_len..].copy_from_slice(&tcp_bytes);
        assert!(
            !valid_syn(&syn, rejected.key),
            "MSS zero is rejected before admission"
        );
        assert_eq!(
            stack
                .handle
                .open(rejected.key, syn.clone())
                .await
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );

        // Also exercise the post-poll binding guard directly, independently of
        // the public parser guard. smoltcp ignores MSS zero and stays Listen.
        let (reply, result) = oneshot::channel();
        stack
            .handle
            .commands
            .send(Command::Open {
                key: rejected.key,
                syn,
                reply,
            })
            .await
            .unwrap();
        assert_eq!(
            result.await.unwrap().unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert!(
            stack.outbound.try_recv().is_err(),
            "rejection emits neither SYN-ACK nor RST"
        );
        assert!(stack.closed.try_recv().is_err());

        // max_flows=1 also proves the rejected socket was removed. The next
        // tuple has the same destination, but must bind its own source port.
        let mut accepted = Client::new(42013);
        let mut stream = stack
            .handle
            .open(accepted.key, accepted.syn())
            .await
            .unwrap();
        accepted.establish(&mut stack).await;
        accepted
            .sockets
            .get_mut::<tcp::Socket>(accepted.socket)
            .send_slice(b"correct peer")
            .unwrap();
        accepted.poll();
        while let Ok(packet) = accepted.outgoing.try_recv() {
            stack.handle.try_send(packet).unwrap();
        }
        let mut body = [0; 12];
        tokio::time::timeout(Duration::from_secs(1), stream.read_exact(&mut body))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&body, b"correct peer");
        stack.shutdown().await;
    }

    #[tokio::test]
    async fn explicit_abort_emits_rst_and_confirms_before_tuple_reuse() {
        let mut stack = TcpStack::new(1420, 1).unwrap();
        let mut client = Client::new(42005);
        let mut stream = stack.handle.open(client.key, client.syn()).await.unwrap();
        client.establish(&mut stack).await;
        stack.handle.abort(client.key).await.unwrap();
        let mut saw_reset = false;
        while let Ok(packet) = stack.outbound.try_recv() {
            let ip = Ipv4Packet::new_checked(&packet).unwrap();
            let tcp = TcpPacket::new_checked(ip.payload()).unwrap();
            saw_reset |= tcp.rst();
        }
        assert!(
            saw_reset,
            "abort completes only after its reset has been queued"
        );
        assert!(
            stack.closed.try_recv().is_err(),
            "explicit abort completes via its reply"
        );
        assert_eq!(stream.read(&mut [0]).await.unwrap(), 0);
        let mut replacement = Client::new(42005);
        let _replacement = stack
            .handle
            .open(replacement.key, replacement.syn())
            .await
            .unwrap();
        stack.shutdown().await;
    }

    #[tokio::test]
    async fn client_reset_closes_byte_stream_and_reports_natural_completion() {
        let mut stack = TcpStack::new(1420, 1).unwrap();
        let mut client = Client::new(42006);
        let mut stream = stack.handle.open(client.key, client.syn()).await.unwrap();
        client.establish(&mut stack).await;
        client.sockets.get_mut::<tcp::Socket>(client.socket).abort();
        client.poll();
        while let Ok(packet) = client.outgoing.try_recv() {
            stack.handle.try_send(packet).unwrap();
        }
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), stack.closed.recv())
                .await
                .unwrap(),
            Some(client.key)
        );
        assert_eq!(stream.read(&mut [0]).await.unwrap(), 0);
        stack.shutdown().await;
    }

    #[tokio::test]
    async fn handshake_reset_cannot_rebind_old_stream_to_next_queued_syn() {
        let mut stack = TcpStack::new(1420, 2).unwrap();
        let mut old = Client::new(42014);
        let mut old_stream = stack.handle.open(old.key, old.syn()).await.unwrap();
        let mut next = Client::new(42015);
        let next_syn = next.syn();
        let mut next_stream = stack.handle.open(next.key, next_syn.clone()).await.unwrap();

        // Both stack sockets are SynReceived; neither client has ACKed. Queue
        // a valid reset immediately followed by another tuple's duplicate SYN
        // without yielding, so they enter the same ingress batch.
        old.sockets.get_mut::<tcp::Socket>(old.socket).abort();
        old.poll();
        let reset = old.outgoing.try_recv().expect("client abort emits RST");
        let ip = Ipv4Packet::new_checked(&reset).unwrap();
        assert!(TcpPacket::new_checked(ip.payload()).unwrap().rst());
        stack.handle.try_send(reset).unwrap();
        stack.handle.try_send(next_syn).unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), stack.closed.recv())
                .await
                .unwrap(),
            Some(old.key)
        );
        assert_eq!(old_stream.read(&mut [0]).await.unwrap(), 0);

        // Discard the old socket's already queued SYN-ACK. Every remaining
        // packet and byte must belong to the new socket's original stream.
        while let Ok(packet) = stack.outbound.try_recv() {
            let ip = Ipv4Packet::new_checked(&packet).unwrap();
            if TcpPacket::new_checked(ip.payload()).unwrap().dst_port() == next.key.client.port() {
                next.device.incoming.push_back(packet);
            }
        }
        next.establish(&mut stack).await;
        next.sockets
            .get_mut::<tcp::Socket>(next.socket)
            .send_slice(b"new flow")
            .unwrap();
        next.poll();
        while let Ok(packet) = next.outgoing.try_recv() {
            stack.handle.try_send(packet).unwrap();
        }
        let mut bytes = [0; 8];
        tokio::time::timeout(Duration::from_secs(1), next_stream.read_exact(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&bytes, b"new flow");
        stack.shutdown().await;
    }

    #[tokio::test]
    async fn unregistered_packets_cannot_create_sockets_or_reset_responses() {
        let mut stack = TcpStack::new(1420, 1).unwrap();
        let mut client = Client::new(42007);
        stack.handle.try_send(client.syn()).unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(25), stack.outbound.recv())
                .await
                .is_err()
        );
        let mut legitimate = Client::new(42008);
        let _stream = stack
            .handle
            .open(legitimate.key, legitimate.syn())
            .await
            .unwrap();
        legitimate.establish(&mut stack).await;
        stack.shutdown().await;
    }

    #[tokio::test]
    async fn concurrent_connections_to_same_destination_keep_distinct_streams() {
        let mut stack = TcpStack::new(1420, 2).unwrap();
        let mut clients = [Client::new(42009), Client::new(42010)];
        let mut first = stack
            .handle
            .open(clients[0].key, clients[0].syn())
            .await
            .unwrap();
        let mut second = stack
            .handle
            .open(clients[1].key, clients[1].syn())
            .await
            .unwrap();
        let mut sent = [false; 2];
        tokio::time::timeout(Duration::from_secs(2), async {
            while !sent.iter().all(|sent| *sent) {
                while let Ok(packet) = stack.outbound.try_recv() {
                    let ip = Ipv4Packet::new_checked(&packet).unwrap();
                    let tcp = TcpPacket::new_checked(ip.payload()).unwrap();
                    let index = usize::from(tcp.dst_port() == clients[1].key.client.port());
                    clients[index].device.incoming.push_back(packet);
                }
                for (index, client) in clients.iter_mut().enumerate() {
                    client.poll();
                    let socket = client.sockets.get_mut::<tcp::Socket>(client.socket);
                    if socket.can_send() && !sent[index] {
                        socket.send_slice(&[index as u8 + 1]).unwrap();
                        sent[index] = true;
                    }
                    client.poll();
                    while let Ok(packet) = client.outgoing.try_recv() {
                        stack.handle.try_send(packet).unwrap();
                    }
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            let mut one = [0];
            let mut two = [0];
            first.read_exact(&mut one).await.unwrap();
            second.read_exact(&mut two).await.unwrap();
            assert_eq!(one, [1]);
            assert_eq!(two, [2]);
        })
        .await
        .expect("distinct client streams receive their own payloads");
        stack.shutdown().await;
    }

    #[tokio::test]
    async fn abort_waits_for_egress_space_before_releasing_socket() {
        let mut stack = TcpStack::with_output_capacity(1420, 1, 1).unwrap();
        let mut client = Client::new(42011);
        let _stream = stack.handle.open(client.key, client.syn()).await.unwrap();
        assert_eq!(
            stack.outbound.len(),
            1,
            "initial SYN-ACK fills bounded output"
        );
        let handle = stack.handle.clone();
        let mut aborted = tokio::spawn(async move { handle.abort(client.key).await });
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut aborted)
                .await
                .is_err()
        );
        stack.outbound.recv().await.unwrap();
        tokio::time::timeout(Duration::from_secs(1), aborted)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let mut saw_reset = false;
        while let Ok(packet) = stack.outbound.try_recv() {
            let ip = Ipv4Packet::new_checked(&packet).unwrap();
            saw_reset |= TcpPacket::new_checked(ip.payload()).unwrap().rst();
        }
        assert!(saw_reset);
        stack.shutdown().await;
    }

    #[test]
    fn output_backpressure_keeps_input_packet_queued() {
        let (output, mut outgoing) = mpsc::channel(1);
        output.try_send(vec![1]).unwrap();
        let mut device = PacketDevice {
            incoming: VecDeque::from([vec![2]]),
            output,
            mtu: 1420,
        };
        assert!(device.receive(Instant::now()).is_none());
        assert_eq!(device.incoming.len(), 1);
        outgoing.try_recv().unwrap();
        let (packet, _) = device.receive(Instant::now()).unwrap();
        assert_eq!(packet.consume(|bytes| bytes.to_vec()), vec![2]);
    }
}
