//! Real TCP connections through a physical uplink, including its default gateway.
//! Linux source policy leases make the real TCP return path compatible with strict
//! rp_filter while leaving packets with the application's VPN source untouched.

use futures::{stream::FuturesUnordered, StreamExt};
use ipnet::Ipv4Net;
use net_route::{Handle, Route};
use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use std::collections::BTreeMap;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;

const CONNECT_TIMEOUT: Duration = Duration::from_millis(800);

pub(crate) struct DirectDialer {
    tun_index: u32,
    vpn_subnet: Ipv4Net,
    stopped: std::sync::atomic::AtomicBool,
    #[cfg(target_os = "linux")]
    policies: linux_policy::Session,
}

impl DirectDialer {
    pub(crate) fn new(tun_index: u32, vpn_subnet: Ipv4Net) -> io::Result<Self> {
        Ok(Self {
            tun_index,
            vpn_subnet,
            stopped: std::sync::atomic::AtomicBool::new(false),
            #[cfg(target_os = "linux")]
            policies: linux_policy::Session::new()?,
        })
    }

    /// Success returns the actual business socket; no second connection or ICMP
    /// inference is used. The caller checks its latest allowed routes before use.
    pub(crate) async fn connect(&self, destination: SocketAddrV4) -> io::Result<DirectStream> {
        if self.stopped.load(std::sync::atomic::Ordering::Acquire) {
            return Err(io::Error::other("direct dialer is closed"));
        }
        if !valid_destination(destination, self.vpn_subnet) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "ineligible direct target",
            ));
        }
        tokio::time::timeout(CONNECT_TIMEOUT, self.connect_inner(destination))
            .await
            .map_err(|_| {
                io::Error::new(io::ErrorKind::TimedOut, "physical TCP connect timed out")
            })?
    }

    async fn connect_inner(&self, destination: SocketAddrV4) -> io::Result<DirectStream> {
        let table = Handle::new()?.list().await?;
        let mut interfaces: Vec<_> = table
            .iter()
            .filter_map(|route| route.ifindex)
            .filter(|index| *index != self.tun_index && *index != 0)
            .collect();
        interfaces.sort_unstable();
        interfaces.dedup();
        interfaces.retain(|index| interface_is_physical(*index));
        // These futures are polled by this connection task. Dropping a winner,
        // timeout, or cancelled dial synchronously drops every losing socket
        // and lease; there are no nested Tokio tasks left for shutdown to join.
        let attempts = FuturesUnordered::new();
        for route in paths(*destination.ip(), &table, &interfaces) {
            let index = route.ifindex.expect("paths includes interface");
            let Ok(source) = source_address(index, *destination.ip()) else {
                continue;
            };
            if self.vpn_subnet.contains(&source) || source == *destination.ip() {
                continue;
            }
            #[cfg(target_os = "linux")]
            let policies = self.policies.clone();
            attempts.push(async move {
                #[cfg(target_os = "linux")]
                let policy = policies.acquire(source, destination, &route).await?;
                let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
                bind_uplink(&socket, index, source)?;
                socket.set_nonblocking(true)?;
                let socket = tokio::net::TcpSocket::from_std_stream(socket.into());
                let stream = socket.connect(SocketAddr::V4(destination)).await?;
                stream.set_nodelay(true)?;
                Ok(DirectStream {
                    stream,
                    #[cfg(target_os = "linux")]
                    _policy: policy,
                })
            });
        }
        first_connected(attempts).await
    }

    /// Call after aborting and joining every connection task. Linux waits for
    /// cancelled acquisitions and all owned policy removals before returning.
    pub(crate) async fn shutdown(&self) -> io::Result<()> {
        self.stopped
            .store(true, std::sync::atomic::Ordering::Release);
        #[cfg(target_os = "linux")]
        self.policies.shutdown().await?;
        Ok(())
    }
}

/// Own all concurrent attempts inside the caller's future so cancellation
/// releases socket resources before that caller can complete its shutdown.
async fn first_connected<T, F>(mut attempts: FuturesUnordered<F>) -> io::Result<T>
where
    F: std::future::Future<Output = io::Result<T>>,
{
    let mut last_error = io::Error::new(io::ErrorKind::NotFound, "no physical IPv4 path");
    while let Some(result) = attempts.next().await {
        match result {
            Ok(stream) => return Ok(stream),
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}

/// Keep platform resources alive for exactly as long as the business socket.
/// Do not extract the inner stream: it would lose its reverse-path lease.
pub(crate) struct DirectStream {
    stream: TcpStream,
    #[cfg(target_os = "linux")]
    _policy: linux_policy::Lease,
}

impl AsyncRead for DirectStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for DirectStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

fn valid_destination(destination: SocketAddrV4, vpn: Ipv4Net) -> bool {
    let ip = destination.ip();
    destination.port() != 0
        && !ip.is_unspecified()
        && !ip.is_loopback()
        && !ip.is_multicast()
        && !ip.is_broadcast()
        && !ip.is_link_local()
        && !vpn.contains(ip)
}

/// Longest physical route on each uplink, including /0. A route is only a
/// candidate; TCP must actually connect before the caller commits the path.
fn paths(ip: Ipv4Addr, table: &[Route], interfaces: &[u32]) -> Vec<Route> {
    let mut best: BTreeMap<u32, &Route> = BTreeMap::new();
    for route in table {
        let (IpAddr::V4(dest), Some(index)) = (route.destination, route.ifindex) else {
            continue;
        };
        if !interfaces.contains(&index) {
            continue;
        }
        #[cfg(target_os = "linux")]
        if route.table != 254 || route.source.is_some() || route.source_prefix != 0 {
            continue;
        }
        if !Ipv4Net::new(dest, route.prefix).is_ok_and(|net| net.contains(&ip)) {
            continue;
        }
        if route
            .gateway
            .is_some_and(|gateway| !matches!(gateway, IpAddr::V4(_)))
        {
            continue;
        }
        if best.get(&index).is_none_or(|old| {
            route.prefix > old.prefix || (route.prefix == old.prefix && metric(route) < metric(old))
        }) {
            best.insert(index, route);
        }
    }
    let mut routes: Vec<_> = best.into_values().cloned().collect();
    routes.sort_by_key(|route| {
        (
            std::cmp::Reverse(route.prefix),
            metric(route),
            route.ifindex,
        )
    });
    routes.truncate(4);
    routes
}

fn metric(route: &Route) -> u32 {
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    {
        route.metric.unwrap_or_default()
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        let _ = route;
        0
    }
}

#[cfg(unix)]
fn interface_name(ifindex: u32) -> io::Result<std::ffi::CString> {
    let mut name = [0 as libc::c_char; libc::IF_NAMESIZE];
    // SAFETY: name is an IF_NAMESIZE-byte writable buffer.
    if ifindex == 0 || unsafe { libc::if_indextoname(ifindex, name.as_mut_ptr()) }.is_null() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "interface unavailable",
        ));
    }
    // SAFETY: a successful if_indextoname call writes a NUL-terminated name.
    Ok(unsafe { std::ffi::CStr::from_ptr(name.as_ptr()) }.to_owned())
}

#[cfg(unix)]
struct InterfaceAddresses(*mut libc::ifaddrs);

#[cfg(unix)]
impl InterfaceAddresses {
    fn new() -> io::Result<Self> {
        let mut addresses = std::ptr::null_mut();
        // SAFETY: getifaddrs initializes the output pointer on success.
        if unsafe { libc::getifaddrs(&mut addresses) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(addresses))
    }
}

#[cfg(unix)]
impl Drop for InterfaceAddresses {
    fn drop(&mut self) {
        // SAFETY: this pointer is owned by this getifaddrs allocation.
        unsafe { libc::freeifaddrs(self.0) };
    }
}

#[cfg(unix)]
fn source_address(ifindex: u32, destination: Ipv4Addr) -> io::Result<Ipv4Addr> {
    let name = interface_name(ifindex)?;
    let addresses = InterfaceAddresses::new()?;
    let mut next = addresses.0;
    let mut best: Option<(u32, Ipv4Addr)> = None;
    while !next.is_null() {
        // SAFETY: getifaddrs returns a valid linked list until it is freed.
        let entry = unsafe { &*next };
        next = entry.ifa_next;
        if entry.ifa_name.is_null() || entry.ifa_addr.is_null() {
            continue;
        }
        // SAFETY: address/name are valid for this live getifaddrs entry.
        let matches_interface =
            unsafe { std::ffi::CStr::from_ptr(entry.ifa_name) } == name.as_c_str();
        let family = unsafe { (*entry.ifa_addr).sa_family } as i32;
        if !matches_interface || family != libc::AF_INET {
            continue;
        }
        // SAFETY: AF_INET addresses have sockaddr_in layout.
        let addr = unsafe { &*entry.ifa_addr.cast::<libc::sockaddr_in>() };
        let ip = Ipv4Addr::from(addr.sin_addr.s_addr.to_ne_bytes());
        if ip.is_unspecified() || ip.is_loopback() || ip.is_link_local() {
            continue;
        }
        let score = if !entry.ifa_netmask.is_null() {
            // SAFETY: the mask of this AF_INET entry has sockaddr_in layout.
            let mask = unsafe { &*entry.ifa_netmask.cast::<libc::sockaddr_in>() };
            let mask = u32::from_be_bytes(mask.sin_addr.s_addr.to_ne_bytes());
            if u32::from(ip) & mask == u32::from(destination) & mask {
                1 + mask.count_ones()
            } else {
                0
            }
        } else {
            0
        };
        if best.is_none_or(|(best_score, _)| score > best_score) {
            best = Some((score, ip));
        }
    }
    best.map(|(_, ip)| ip).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            "uplink has no IPv4 address",
        )
    })
}

#[cfg(target_os = "linux")]
fn interface_is_physical(ifindex: u32) -> bool {
    let Ok(name) = interface_name(ifindex) else {
        return false;
    };
    let Ok(name) = name.to_str() else {
        return false;
    };
    let base = std::path::Path::new("/sys/class/net").join(name);
    // TUN/TAP, veth and software bridges have no hardware device. Requiring one
    // prevents an Ethernet-looking VPN adapter from passing this check.
    if !base.join("device").exists() {
        return false;
    }
    let ethernet =
        std::fs::read_to_string(base.join("type")).is_ok_and(|value| value.trim() == "1"); // ARPHRD_ETHER, including Wi-Fi.
    let flags = std::fs::read_to_string(base.join("flags"))
        .ok()
        .and_then(|value| u32::from_str_radix(value.trim().trim_start_matches("0x"), 16).ok())
        .unwrap_or(0);
    ethernet
        && flags & libc::IFF_UP as u32 != 0
        && flags & (libc::IFF_LOOPBACK | libc::IFF_POINTOPOINT) as u32 == 0
}

#[cfg(target_os = "macos")]
fn interface_is_physical(ifindex: u32) -> bool {
    if ifindex == 0 {
        return false;
    }
    let Ok(name) = interface_name(ifindex) else {
        return false;
    };
    let Ok(name) = name.to_str() else {
        return false;
    };
    // macOS Ethernet/Wi-Fi (including USB adapters) use enN. Link type alone
    // also accepts software TAP adapters, which must not prove LAN access.
    if !name.strip_prefix("en").is_some_and(|suffix| {
        !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
    }) {
        return false;
    }
    let Ok(addresses) = InterfaceAddresses::new() else {
        return false;
    };
    let mut next = addresses.0;
    while !next.is_null() {
        // SAFETY: getifaddrs allocation owns each entry and address.
        let entry = unsafe { &*next };
        next = entry.ifa_next;
        if entry.ifa_addr.is_null()
            || unsafe { (*entry.ifa_addr).sa_family } as i32 != libc::AF_LINK
        {
            continue;
        }
        // SAFETY: AF_LINK addresses have sockaddr_dl layout on macOS.
        let link = unsafe { &*entry.ifa_addr.cast::<libc::sockaddr_dl>() };
        if u32::from(link.sdl_index) != ifindex {
            continue;
        }
        // IFT_ETHER / IFT_IEEE80211. utun uses a different link type.
        return matches!(link.sdl_type, 6 | 71)
            && entry.ifa_flags & libc::IFF_UP as u32 != 0
            && entry.ifa_flags & (libc::IFF_LOOPBACK | libc::IFF_POINTOPOINT) as u32 == 0;
    }
    false
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn bind_uplink(socket: &Socket, ifindex: u32, source: Ipv4Addr) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    socket.bind_device(Some(interface_name(ifindex)?.as_bytes()))?;
    #[cfg(target_os = "macos")]
    socket
        .bind_device_by_index_v4(Some(std::num::NonZeroU32::new(ifindex).ok_or_else(
            || io::Error::new(io::ErrorKind::InvalidInput, "zero interface index"),
        )?))?;
    socket.bind(&SockAddr::from(SocketAddrV4::new(source, 0)))
}

#[cfg(target_os = "windows")]
fn interface_is_physical(ifindex: u32) -> bool {
    use windows_sys::Win32::NetworkManagement::IpHelper::{GetIfEntry2, MIB_IF_ROW2};
    // SAFETY: zero initializes the C structure, then InterfaceIndex selects it.
    let mut row: MIB_IF_ROW2 = unsafe { std::mem::zeroed() };
    row.InterfaceIndex = ifindex;
    if ifindex == 0 || unsafe { GetIfEntry2(&mut row) } != 0 {
        return false;
    }
    // HardwareInterface is the low bit, OperStatus 1 means Up.
    row.InterfaceAndOperStatusFlags._bitfield & 1 != 0
        && row.OperStatus == 1
        && matches!(row.Type, 6 | 71)
}

#[cfg(target_os = "windows")]
fn source_address(ifindex: u32, destination: Ipv4Addr) -> io::Result<Ipv4Addr> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{GetBestRoute2, MIB_IPFORWARD_ROW2};
    use windows_sys::Win32::Networking::WinSock::{AF_INET, SOCKADDR_INET};
    // SAFETY: these are plain C output structures, initialized before FFI use.
    let (mut address, mut route, mut source): (SOCKADDR_INET, MIB_IPFORWARD_ROW2, SOCKADDR_INET) =
        unsafe { std::mem::zeroed() };
    address.Ipv4.sin_family = AF_INET;
    address.Ipv4.sin_addr.S_un.S_addr = u32::from_ne_bytes(destination.octets());
    // A constrained lookup selects the source on the physical interface even
    // when the VPN owns a more-specific route on a different interface.
    let result = unsafe {
        GetBestRoute2(
            std::ptr::null(),
            ifindex,
            std::ptr::null(),
            &address,
            0,
            &mut route,
            &mut source,
        )
    };
    if result != 0 {
        return Err(io::Error::from_raw_os_error(result as i32));
    }
    // SAFETY: GetBestRoute2 returns a source in the requested address family.
    if unsafe { source.si_family } != AF_INET || route.InterfaceIndex != ifindex {
        return Err(io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            "no physical IPv4 route",
        ));
    }
    Ok(Ipv4Addr::from(
        unsafe { source.Ipv4.sin_addr.S_un.S_addr }.to_ne_bytes(),
    ))
}

#[cfg(target_os = "windows")]
fn bind_uplink(socket: &Socket, ifindex: u32, source: Ipv4Addr) -> io::Result<()> {
    use windows_sys::Win32::Networking::WinSock::IP_UNICAST_IF;
    // Microsoft specifies network byte order for IP_UNICAST_IF's index.
    set_windows_ip_option(socket, IP_UNICAST_IF, ifindex.to_be())?;
    socket.bind(&SockAddr::from(SocketAddrV4::new(source, 0)))?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn set_windows_ip_option(socket: &Socket, option: i32, value: u32) -> io::Result<()> {
    use std::os::windows::io::AsRawSocket;
    use windows_sys::Win32::Networking::WinSock::{
        setsockopt, WSAGetLastError, IPPROTO_IP, SOCKET_ERROR,
    };
    // SAFETY: socket is live and value points to a DWORD for this synchronous call.
    let result = unsafe {
        setsockopt(
            socket.as_raw_socket() as _,
            IPPROTO_IP,
            option,
            (&value as *const u32).cast(),
            std::mem::size_of::<u32>() as i32,
        )
    };
    if result == SOCKET_ERROR {
        return Err(io::Error::from_raw_os_error(unsafe { WSAGetLastError() }));
    }
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn interface_is_physical(_ifindex: u32) -> bool {
    false
}

#[cfg(not(any(unix, target_os = "windows")))]
fn source_address(_ifindex: u32, _destination: Ipv4Addr) -> io::Result<Ipv4Addr> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "unsupported platform",
    ))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn bind_uplink(_socket: &Socket, _ifindex: u32, _source: Ipv4Addr) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "unsupported platform",
    ))
}

#[cfg(any(target_os = "linux", test))]
mod linux_policy {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{mpsc, Arc, Mutex};
    use tokio::sync::oneshot;

    // UAPI linux/rtnetlink.h and linux/fib_rules.h. Explicit byte encoding avoids
    // alignment/ABI casts and also lets the transaction tests run on macOS.
    const NEW_ROUTE: u16 = 24;
    const DEL_ROUTE: u16 = 25;
    const GET_ROUTE: u16 = 26;
    const NEW_RULE: u16 = 32;
    const DEL_RULE: u16 = 33;
    const GET_RULE: u16 = 34;
    const RTA_DST: u16 = 1;
    const RTA_OIF: u16 = 4;
    const RTA_GATEWAY: u16 = 5;
    const RTA_PRIORITY: u16 = 6;
    const RTA_TABLE: u16 = 15;
    const FRA_DST: u16 = 1;
    const FRA_SRC: u16 = 2;
    const FRA_PRIORITY: u16 = 6;
    const FRA_TABLE: u16 = 15;
    const FRA_PROTOCOL: u16 = 21;
    const PROTOCOL: u8 = 242;
    const MAX_POLICIES: usize = 256;

    #[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
    struct Key {
        source: Ipv4Addr,
        destination: Ipv4Addr,
        ifindex: u32,
        gateway: Option<Ipv4Addr>,
    }

    pub(super) struct Lease {
        sender: mpsc::Sender<Request>,
        key: Key,
    }

    impl Drop for Lease {
        fn drop(&mut self) {
            let _ = self.sender.send(Request::Release(self.key));
        }
    }

    #[derive(Clone)]
    pub(super) struct Session(Arc<SessionInner>);

    struct SessionInner {
        sender: mpsc::Sender<Request>,
        stopped: Arc<AtomicBool>,
        join: Mutex<Option<std::thread::JoinHandle<io::Result<()>>>>,
        shutdown_lock: tokio::sync::Mutex<()>,
        result: Mutex<Option<Result<(), String>>>,
    }

    impl Drop for SessionInner {
        fn drop(&mut self) {
            // Emergency teardown uses the same owning worker. Normal shutdown
            // explicitly joins it and reports any failed deletion to the caller.
            self.stopped.store(true, Ordering::Release);
            let _ = self.sender.send(Request::Shutdown);
        }
    }

    enum Request {
        Acquire(
            Key,
            mpsc::Sender<Request>,
            oneshot::Sender<io::Result<Lease>>,
        ),
        Release(Key),
        Shutdown,
    }

    impl Session {
        #[cfg(target_os = "linux")]
        pub(super) fn new() -> io::Result<Self> {
            Self::with_backend(Netlink::new()?)
        }

        fn with_backend(backend: impl Backend + Send + 'static) -> io::Result<Self> {
            let (sender, receiver) = mpsc::channel();
            let stopped = Arc::new(AtomicBool::new(false));
            let worker_stopped = stopped.clone();
            let join = std::thread::Builder::new()
                .name("vpn-direct-policy".into())
                .spawn(move || run(backend, receiver, worker_stopped))?;
            Ok(Self(Arc::new(SessionInner {
                sender,
                stopped,
                join: Mutex::new(Some(join)),
                shutdown_lock: tokio::sync::Mutex::new(()),
                result: Mutex::new(None),
            })))
        }

        pub(super) async fn acquire(
            &self,
            source: Ipv4Addr,
            destination: SocketAddrV4,
            route: &Route,
        ) -> io::Result<Lease> {
            if self.0.stopped.load(Ordering::Acquire) {
                return Err(io::Error::other("direct policy session is closed"));
            }
            let key = Key {
                source,
                destination: *destination.ip(),
                ifindex: route
                    .ifindex
                    .ok_or_else(|| io::Error::other("missing physical interface"))?,
                gateway: route.gateway.and_then(|ip| match ip {
                    IpAddr::V4(ip) if !ip.is_unspecified() => Some(ip),
                    _ => None,
                }),
            };
            let (reply, result) = oneshot::channel();
            self.0
                .sender
                .send(Request::Acquire(key, self.0.sender.clone(), reply))
                .map_err(|_| io::Error::other("direct policy worker stopped"))?;
            result
                .await
                .map_err(|_| io::Error::other("direct policy worker stopped"))?
        }

        pub(super) async fn shutdown(&self) -> io::Result<()> {
            let _lock = self.0.shutdown_lock.lock().await;
            if let Some(result) = self.0.result.lock().unwrap().clone() {
                return result.map_err(io::Error::other);
            }
            self.0.stopped.store(true, Ordering::Release);
            let _ = self.0.sender.send(Request::Shutdown);
            // A cancelled shutdown must not lose the JoinHandle. Polling
            // is_finished is bounded by the worker's per-request netlink limits.
            loop {
                let finished = self
                    .0
                    .join
                    .lock()
                    .unwrap()
                    .as_ref()
                    .is_none_or(|join| join.is_finished());
                if finished {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            let joined = self.0.join.lock().unwrap().take().map(|join| join.join());
            let result = match joined {
                Some(Ok(result)) => result,
                Some(Err(_)) => Err(io::Error::other("direct policy cleanup worker panicked")),
                None => Err(io::Error::other("direct policy cleanup status unavailable")),
            };
            *self.0.result.lock().unwrap() =
                Some(result.as_ref().map(|_| ()).map_err(ToString::to_string));
            result
        }
    }

    /// A transport error after send may have applied the operation. The caller
    /// then keeps its exact ownership key until deletion is acknowledged.
    struct Failure {
        error: io::Error,
        uncertain: bool,
    }

    trait Backend {
        fn dump(&mut self, kind: u16) -> io::Result<Vec<Vec<u8>>>;
        fn change(&mut self, kind: u16, payload: &[u8]) -> Result<(), Failure>;
    }

    struct Policy {
        route: Vec<u8>,
        rule: Vec<u8>,
        route_owned: bool,
        rule_owned: bool,
        references: usize,
    }

    impl Policy {
        fn new(key: Key, table: u32, priority: u32) -> Self {
            // Only this physical source -> exact destination pair can select
            // this table. The application's VPN-source packets do not match.
            // Linux __fib_validate_source reverses saddr/daddr for its lookup,
            // so this same rule fixes strict rp_filter for real TCP replies.
            let mut route = vec![
                2,
                32,
                0,
                0,
                0,
                PROTOCOL,
                if key.gateway.is_some() { 0 } else { 253 },
                1,
                0,
                0,
                0,
                0,
            ];
            if key.gateway.is_some() {
                route[8..12].copy_from_slice(&4u32.to_ne_bytes()); // RTNH_F_ONLINK
            }
            attr(&mut route, RTA_DST, &key.destination.octets());
            attr(&mut route, RTA_OIF, &key.ifindex.to_ne_bytes());
            attr(&mut route, RTA_TABLE, &table.to_ne_bytes());
            // A private metric supplements the private table and protocol in
            // the precise delete key; never use a prefix-only route deletion.
            attr(&mut route, RTA_PRIORITY, &table.to_ne_bytes());
            if let Some(gateway) = key.gateway {
                attr(&mut route, RTA_GATEWAY, &gateway.octets());
            }
            let mut rule = vec![2, 32, 32, 0, 0, 0, 0, 1, 0, 0, 0, 0];
            attr(&mut rule, FRA_SRC, &key.source.octets());
            attr(&mut rule, FRA_DST, &key.destination.octets());
            attr(&mut rule, FRA_TABLE, &table.to_ne_bytes());
            attr(&mut rule, FRA_PRIORITY, &priority.to_ne_bytes());
            attr(&mut rule, FRA_PROTOCOL, &[PROTOCOL]);
            Self {
                route,
                rule,
                route_owned: false,
                rule_owned: false,
                references: 0,
            }
        }

        fn install(&mut self, backend: &mut impl Backend) -> io::Result<()> {
            match backend.change(NEW_ROUTE, &self.route) {
                Ok(()) => self.route_owned = true,
                Err(failure) => {
                    self.route_owned = failure.uncertain;
                    return Err(failure.error);
                }
            }
            match backend.change(NEW_RULE, &self.rule) {
                Ok(()) => self.rule_owned = true,
                Err(failure) => {
                    self.rule_owned = failure.uncertain;
                    return Err(failure.error);
                }
            }
            Ok(())
        }

        fn cleanup(&mut self, backend: &mut impl Backend) -> io::Result<()> {
            let mut error = None;
            // Withdraw the selector first, then its private host route. If the
            // rule delete fails, still remove the route to disable that path.
            for (owned, kind, payload) in [
                (&mut self.rule_owned, DEL_RULE, &self.rule),
                (&mut self.route_owned, DEL_ROUTE, &self.route),
            ] {
                if !*owned {
                    continue;
                }
                match backend.change(kind, payload) {
                    Ok(()) => *owned = false,
                    Err(failure) if matches!(failure.error.raw_os_error(), Some(2 | 3)) => {
                        *owned = false
                    }
                    Err(failure) => error = Some(failure.error),
                }
            }
            error.map_or(Ok(()), Err)
        }

        fn still_owned(&self) -> bool {
            self.route_owned || self.rule_owned
        }
    }

    fn allocate(backend: &mut impl Backend) -> io::Result<(u32, u32)> {
        let mut tables = HashSet::new();
        let mut priorities = HashSet::new();
        for kind in [GET_ROUTE, GET_RULE] {
            for payload in backend.dump(kind)? {
                if payload.len() < 12 {
                    return Err(io::Error::other("truncated routing dump"));
                }
                let attributes = attrs(&payload[12..])?;
                let table = attributes
                    .iter()
                    .find(|(kind, _)| *kind == RTA_TABLE)
                    .and_then(|(_, value)| u32_value(value))
                    .unwrap_or(u32::from(payload[4]));
                tables.insert(table);
                if kind == GET_RULE {
                    if let Some(priority) = attributes
                        .iter()
                        .find(|(kind, _)| *kind == FRA_PRIORITY)
                        .and_then(|(_, value)| u32_value(value))
                    {
                        priorities.insert(priority);
                    }
                }
            }
        }
        let seed = rand::random::<u32>();
        let table = (0..1024)
            .map(|offset| 0x5e00_0000 | (seed.wrapping_add(offset) & 0x00ff_ffff))
            .find(|table| !tables.contains(table))
            .ok_or_else(|| io::Error::other("no unused direct policy table"))?;
        // Preserve the kernel local rule (0); select before main (32766).
        let priority = (10_000..30_000)
            .find(|priority| !priorities.contains(priority))
            .ok_or_else(|| io::Error::other("no unused direct policy priority"))?;
        Ok((table, priority))
    }

    fn run(
        mut backend: impl Backend,
        receiver: mpsc::Receiver<Request>,
        stopped: Arc<AtomicBool>,
    ) -> io::Result<()> {
        let mut policies: HashMap<Key, Policy> = HashMap::new();
        while let Ok(request) = receiver.recv() {
            match request {
                Request::Acquire(key, sender, reply) => {
                    if reply.is_closed() || stopped.load(Ordering::Acquire) {
                        continue;
                    }
                    let result = (|| {
                        if let Some(policy) = policies.get_mut(&key) {
                            if policy.references > 0 {
                                policy.references += 1;
                                return Ok(Lease { sender, key });
                            }
                            policy.cleanup(&mut backend)?;
                            policies.remove(&key);
                        }
                        if policies.len() >= MAX_POLICIES {
                            return Err(io::Error::other("direct policy capacity reached"));
                        }
                        let (table, priority) = allocate(&mut backend)?;
                        if reply.is_closed() || stopped.load(Ordering::Acquire) {
                            return Err(io::Error::other("direct policy acquisition cancelled"));
                        }
                        let mut policy = Policy::new(key, table, priority);
                        let result = policy.install(&mut backend);
                        if let Err(error) = result {
                            if let Err(cleanup) = policy.cleanup(&mut backend) {
                                tracing::warn!(%cleanup, "物理直连策略安装失败，保留清理记录");
                            }
                            if policy.still_owned() {
                                policies.insert(key, policy);
                            }
                            return Err(error);
                        }
                        policy.references = 1;
                        policies.insert(key, policy);
                        Ok(Lease { sender, key })
                    })();
                    // If cancellation raced creation, send returns the lease;
                    // dropping it enqueues Release on this same owning worker.
                    let _ = reply.send(result);
                }
                Request::Release(key) => {
                    if let Some(policy) = policies.get_mut(&key) {
                        policy.references = policy.references.saturating_sub(1);
                        if policy.references == 0 {
                            if let Err(error) = policy.cleanup(&mut backend) {
                                tracing::warn!(%error, "物理直连策略删除失败，将在关闭时重试");
                            }
                            if !policy.still_owned() {
                                policies.remove(&key);
                            }
                        }
                    }
                }
                Request::Shutdown => break,
            }
        }
        // One retry handles interrupted/deferred kernel operations. Failures
        // remain visible; the caller must report cleanup failure, not success.
        for _ in 0..2 {
            for policy in policies.values_mut() {
                let _ = policy.cleanup(&mut backend);
            }
            policies.retain(|_, policy| policy.still_owned());
            if policies.is_empty() {
                return Ok(());
            }
        }
        Err(io::Error::other(format!(
            "{} physical direct policy resources could not be removed",
            policies.len()
        )))
    }

    fn attr(message: &mut Vec<u8>, kind: u16, value: &[u8]) {
        let len = 4 + value.len();
        message.extend_from_slice(&(len as u16).to_ne_bytes());
        message.extend_from_slice(&kind.to_ne_bytes());
        message.extend_from_slice(value);
        message.resize(message.len() + ((4 - len % 4) % 4), 0);
    }

    fn attrs(mut bytes: &[u8]) -> io::Result<Vec<(u16, &[u8])>> {
        let mut result = Vec::new();
        while !bytes.is_empty() {
            if bytes.len() < 4 {
                return Err(io::Error::other("truncated route attribute"));
            }
            let len = usize::from(u16::from_ne_bytes(bytes[..2].try_into().unwrap()));
            let kind = u16::from_ne_bytes(bytes[2..4].try_into().unwrap()) & 0x3fff;
            let aligned = (len + 3) & !3;
            if len < 4 || aligned > bytes.len() {
                return Err(io::Error::other("invalid route attribute length"));
            }
            result.push((kind, &bytes[4..len]));
            bytes = &bytes[aligned..];
        }
        Ok(result)
    }

    fn u32_value(value: &[u8]) -> Option<u32> {
        Some(u32::from_ne_bytes(value.try_into().ok()?))
    }

    #[cfg(target_os = "linux")]
    struct Netlink {
        socket: Socket,
        sequence: u32,
    }

    #[cfg(target_os = "linux")]
    impl Netlink {
        fn new() -> io::Result<Self> {
            let socket = Socket::new(
                Domain::from(libc::AF_NETLINK),
                Type::RAW,
                Some(Protocol::from(libc::NETLINK_ROUTE)),
            )?;
            socket.set_read_timeout(Some(Duration::from_millis(100)))?;
            socket.set_write_timeout(Some(Duration::from_millis(100)))?;
            let mut storage = socket2::SockAddrStorage::zeroed();
            // SAFETY: this storage fits sockaddr_nl and is fully zero initialized.
            let address = unsafe { storage.view_as::<libc::sockaddr_nl>() };
            address.nl_family = libc::AF_NETLINK as u16;
            // nl_pid=0/nl_groups=0 addresses only the kernel, not multicast peers.
            let address =
                unsafe { SockAddr::new(storage, std::mem::size_of::<libc::sockaddr_nl>() as _) };
            socket.connect(&address)?;
            Ok(Self {
                socket,
                sequence: 0,
            })
        }

        fn request(
            &mut self,
            kind: u16,
            payload: &[u8],
            dump: bool,
        ) -> Result<Vec<Vec<u8>>, Failure> {
            let failure = |error, uncertain| Failure { error, uncertain };
            self.sequence = self.sequence.wrapping_add(1);
            let mut message = Vec::new();
            message.extend_from_slice(&((16 + payload.len()) as u32).to_ne_bytes());
            message.extend_from_slice(&kind.to_ne_bytes());
            let flags: u16 = if dump {
                1 | 0x300
            } else if matches!(kind, NEW_ROUTE | NEW_RULE) {
                1 | 4 | 0x600
            } else {
                1 | 4
            };
            message.extend_from_slice(&flags.to_ne_bytes());
            message.extend_from_slice(&self.sequence.to_ne_bytes());
            message.extend_from_slice(&0u32.to_ne_bytes());
            message.extend_from_slice(payload);
            let sent = self.socket.send(&message).map_err(|e| failure(e, false))?;
            if sent != message.len() {
                return Err(failure(io::Error::other("short netlink send"), true));
            }
            let deadline = std::time::Instant::now() + Duration::from_millis(200);
            let mut results = Vec::new();
            let mut total = 0usize;
            let mut buffer = vec![std::mem::MaybeUninit::new(0u8); 65536];
            loop {
                if std::time::Instant::now() >= deadline {
                    return Err(failure(
                        io::Error::new(io::ErrorKind::TimedOut, "netlink acknowledgment timed out"),
                        true,
                    ));
                }
                let (len, peer) = self
                    .socket
                    .recv_from(&mut buffer)
                    .map_err(|e| failure(e, true))?;
                if len == buffer.len() {
                    return Err(failure(io::Error::other("oversize netlink reply"), true));
                }
                if peer.family() != libc::AF_NETLINK as u16
                    || peer.len() < std::mem::size_of::<libc::sockaddr_nl>() as _
                {
                    return Err(failure(io::Error::other("invalid netlink sender"), true));
                }
                // SAFETY: family and length establish sockaddr_nl layout.
                if unsafe { (*peer.as_ptr().cast::<libc::sockaddr_nl>()).nl_pid } != 0 {
                    continue;
                }
                // SAFETY: recv_from initializes exactly len bytes.
                let mut bytes =
                    unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), len) };
                while !bytes.is_empty() {
                    if bytes.len() < 16 {
                        return Err(failure(io::Error::other("truncated netlink header"), true));
                    }
                    let size = u32_value(&bytes[..4]).unwrap() as usize;
                    if size < 16 || size > bytes.len() {
                        return Err(failure(io::Error::other("invalid netlink length"), true));
                    }
                    let kind = u16::from_ne_bytes(bytes[4..6].try_into().unwrap());
                    let flags = u16::from_ne_bytes(bytes[6..8].try_into().unwrap());
                    let sequence = u32_value(&bytes[8..12]).unwrap();
                    let body = &bytes[16..size];
                    if sequence == self.sequence {
                        if flags & 0x10 != 0 {
                            return Err(failure(
                                io::Error::other("netlink dump interrupted"),
                                true,
                            ));
                        }
                        if kind == 2 {
                            let Some(error) = u32_value(body.get(..4).unwrap_or_default()) else {
                                return Err(failure(
                                    io::Error::other("truncated netlink acknowledgment"),
                                    true,
                                ));
                            };
                            if error == 0 {
                                return Ok(results);
                            }
                            return Err(failure(
                                io::Error::from_raw_os_error(-(error as i32)),
                                false,
                            ));
                        }
                        if kind == 3 {
                            if let Some(error) = body.get(..4).and_then(u32_value) {
                                if error != 0 {
                                    return Err(failure(
                                        io::Error::from_raw_os_error(-(error as i32)),
                                        false,
                                    ));
                                }
                            }
                            return Ok(results);
                        }
                        if kind == 4 {
                            return Err(failure(io::Error::other("netlink receive overrun"), true));
                        }
                        if matches!(kind, NEW_ROUTE | NEW_RULE) {
                            total += body.len();
                            if total > 4 * 1024 * 1024 {
                                return Err(failure(
                                    io::Error::other("routing dump too large"),
                                    true,
                                ));
                            }
                            results.push(body.to_vec());
                        }
                    }
                    let aligned = (size + 3) & !3;
                    if aligned > bytes.len() {
                        return Err(failure(io::Error::other("truncated netlink padding"), true));
                    }
                    bytes = &bytes[aligned..];
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    impl Backend for Netlink {
        fn dump(&mut self, kind: u16) -> io::Result<Vec<Vec<u8>>> {
            let mut payload = [0; 12];
            payload[0] = 2;
            self.request(kind, &payload, true)
                .map_err(|failure| failure.error)
        }
        fn change(&mut self, kind: u16, payload: &[u8]) -> Result<(), Failure> {
            self.request(kind, payload, false).map(|_| ())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::collections::VecDeque;

        #[derive(Default)]
        struct State {
            calls: Vec<(u16, Vec<u8>)>,
            failures: VecDeque<(u16, bool, i32)>,
            dump_rules: Vec<Vec<u8>>,
            dump_routes: Vec<Vec<u8>>,
            deletes_always_fail: bool,
        }

        struct Mock(Arc<Mutex<State>>);

        impl Backend for Mock {
            fn dump(&mut self, kind: u16) -> io::Result<Vec<Vec<u8>>> {
                let state = self.0.lock().unwrap();
                Ok(if kind == GET_RULE {
                    state.dump_rules.clone()
                } else {
                    state.dump_routes.clone()
                })
            }
            fn change(&mut self, kind: u16, payload: &[u8]) -> Result<(), Failure> {
                let mut state = self.0.lock().unwrap();
                state.calls.push((kind, payload.to_vec()));
                if state
                    .failures
                    .front()
                    .is_some_and(|(expected, _, _)| *expected == kind)
                {
                    let (_, uncertain, errno) = state.failures.pop_front().unwrap();
                    return Err(Failure {
                        error: io::Error::from_raw_os_error(errno),
                        uncertain,
                    });
                }
                if state.deletes_always_fail && matches!(kind, DEL_RULE | DEL_ROUTE) {
                    return Err(Failure {
                        error: io::Error::from_raw_os_error(1),
                        uncertain: false,
                    });
                }
                Ok(())
            }
        }

        fn key() -> Key {
            Key {
                source: "192.168.187.30".parse().unwrap(),
                destination: "192.168.188.111".parse().unwrap(),
                ifindex: 3,
                gateway: Some("192.168.187.1".parse().unwrap()),
            }
        }

        fn route() -> Route {
            Route::new(Ipv4Addr::UNSPECIFIED.into(), 0)
                .with_ifindex(3)
                .with_gateway("192.168.187.1".parse().unwrap())
        }

        #[test]
        fn source_rule_is_exact_and_does_not_select_vpn_source_packets() {
            let policy = Policy::new(key(), 0x5e123456, 10_000);
            assert_eq!(&policy.rule[..3], &[2, 32, 32]);
            let attributes = attrs(&policy.rule[12..]).unwrap();
            let source = attributes
                .iter()
                .find(|(kind, _)| *kind == FRA_SRC)
                .unwrap()
                .1;
            let destination = attributes
                .iter()
                .find(|(kind, _)| *kind == FRA_DST)
                .unwrap()
                .1;
            assert_eq!(source, key().source.octets());
            assert_eq!(destination, key().destination.octets());
            assert_ne!(source, Ipv4Addr::new(10, 8, 0, 2).octets());
            assert_eq!(u32_value(&policy.route[8..12]), Some(4));
            let route_attrs = attrs(&policy.route[12..]).unwrap();
            assert_eq!(
                route_attrs
                    .iter()
                    .find(|(kind, _)| *kind == RTA_GATEWAY)
                    .unwrap()
                    .1,
                key().gateway.unwrap().octets()
            );
        }

        #[test]
        fn failed_exclusive_add_never_owns_or_deletes_existing_route() {
            let state = Arc::new(Mutex::new(State::default()));
            state
                .lock()
                .unwrap()
                .failures
                .push_back((NEW_ROUTE, false, 17));
            let mut backend = Mock(state.clone());
            let mut policy = Policy::new(key(), 0x5e123456, 10_000);
            assert!(policy.install(&mut backend).is_err());
            assert!(policy.cleanup(&mut backend).is_ok());
            assert!(!policy.still_owned());
            assert_eq!(
                state
                    .lock()
                    .unwrap()
                    .calls
                    .iter()
                    .map(|(kind, _)| *kind)
                    .collect::<Vec<_>>(),
                [NEW_ROUTE]
            );
        }

        #[test]
        fn partial_install_removes_only_acknowledged_or_uncertain_ownership() {
            for uncertain in [false, true] {
                let state = Arc::new(Mutex::new(State::default()));
                state
                    .lock()
                    .unwrap()
                    .failures
                    .push_back((NEW_RULE, uncertain, 1));
                let mut backend = Mock(state.clone());
                let mut policy = Policy::new(key(), 0x5e123456, 10_000);
                assert!(policy.install(&mut backend).is_err());
                policy.cleanup(&mut backend).unwrap();
                let state = state.lock().unwrap();
                let kinds: Vec<_> = state.calls.iter().map(|(kind, _)| *kind).collect();
                assert_eq!(
                    kinds,
                    if uncertain {
                        vec![NEW_ROUTE, NEW_RULE, DEL_RULE, DEL_ROUTE]
                    } else {
                        vec![NEW_ROUTE, NEW_RULE, DEL_ROUTE]
                    }
                );
                assert_eq!(
                    state.calls.first().unwrap().1,
                    state.calls.last().unwrap().1
                );
                assert!(!policy.still_owned());
            }
        }

        #[test]
        fn cleanup_failure_is_retained_and_retried_without_forgetting_ownership() {
            let state = Arc::new(Mutex::new(State::default()));
            let mut backend = Mock(state.clone());
            let mut policy = Policy::new(key(), 0x5e123456, 10_000);
            policy.install(&mut backend).unwrap();
            state
                .lock()
                .unwrap()
                .failures
                .push_back((DEL_RULE, false, 1));
            assert!(policy.cleanup(&mut backend).is_err());
            assert!(policy.rule_owned);
            assert!(!policy.route_owned);
            policy.cleanup(&mut backend).unwrap();
            assert!(!policy.still_owned());
        }

        #[tokio::test]
        async fn sessions_share_policy_until_last_lease_then_shutdown_joins_cleanup() {
            let state = Arc::new(Mutex::new(State::default()));
            let session = Session::with_backend(Mock(state.clone())).unwrap();
            let destination = SocketAddrV4::new(key().destination, 8443);
            let first = session
                .acquire(key().source, destination, &route())
                .await
                .unwrap();
            let second = session
                .acquire(
                    key().source,
                    SocketAddrV4::new(key().destination, 443),
                    &route(),
                )
                .await
                .unwrap();
            drop(first);
            // A third acquire is an ordered barrier after Release(first).
            let third = session
                .acquire(key().source, destination, &route())
                .await
                .unwrap();
            assert_eq!(state.lock().unwrap().calls.len(), 2);
            drop(second);
            drop(third);
            session.shutdown().await.unwrap();
            assert_eq!(
                state
                    .lock()
                    .unwrap()
                    .calls
                    .iter()
                    .map(|(kind, _)| *kind)
                    .collect::<Vec<_>>(),
                [NEW_ROUTE, NEW_RULE, DEL_RULE, DEL_ROUTE]
            );
            session.shutdown().await.unwrap();
            assert!(session
                .acquire(key().source, destination, &route())
                .await
                .is_err());
        }

        #[tokio::test]
        async fn cancelled_acquisition_cannot_leave_a_policy_after_shutdown() {
            let state = Arc::new(Mutex::new(State::default()));
            let session = Session::with_backend(Mock(state.clone())).unwrap();
            let (reply, result) = oneshot::channel();
            session
                .0
                .sender
                .send(Request::Acquire(key(), session.0.sender.clone(), reply))
                .unwrap();
            drop(result);
            session.shutdown().await.unwrap();
            let calls = &state.lock().unwrap().calls;
            let adds = calls.iter().filter(|(kind, _)| *kind == NEW_RULE).count();
            let deletes = calls.iter().filter(|(kind, _)| *kind == DEL_RULE).count();
            assert_eq!(adds, deletes);
        }

        #[tokio::test]
        async fn cancellation_during_rule_creation_releases_the_completed_transaction() {
            struct Paused {
                backend: Mock,
                entered: Option<oneshot::Sender<()>>,
                resume: mpsc::Receiver<()>,
            }
            impl Backend for Paused {
                fn dump(&mut self, kind: u16) -> io::Result<Vec<Vec<u8>>> {
                    self.backend.dump(kind)
                }
                fn change(&mut self, kind: u16, payload: &[u8]) -> Result<(), Failure> {
                    if kind == NEW_RULE {
                        if let Some(entered) = self.entered.take() {
                            let _ = entered.send(());
                            self.resume.recv_timeout(Duration::from_secs(2)).unwrap();
                        }
                    }
                    self.backend.change(kind, payload)
                }
            }
            let state = Arc::new(Mutex::new(State::default()));
            let (entered, waiting) = oneshot::channel();
            let (resume, gate) = mpsc::channel();
            let session = Session::with_backend(Paused {
                backend: Mock(state.clone()),
                entered: Some(entered),
                resume: gate,
            })
            .unwrap();
            let task_session = session.clone();
            let task = tokio::spawn(async move {
                task_session
                    .acquire(
                        key().source,
                        SocketAddrV4::new(key().destination, 8443),
                        &route(),
                    )
                    .await
            });
            waiting.await.unwrap();
            task.abort();
            assert!(matches!(task.await, Err(error) if error.is_cancelled()));
            resume.send(()).unwrap();
            session.shutdown().await.unwrap();
            assert_eq!(
                state
                    .lock()
                    .unwrap()
                    .calls
                    .iter()
                    .map(|(kind, _)| *kind)
                    .collect::<Vec<_>>(),
                [NEW_ROUTE, NEW_RULE, DEL_RULE, DEL_ROUTE]
            );
        }

        #[tokio::test]
        async fn shutdown_reports_and_remembers_cleanup_failure() {
            let state = Arc::new(Mutex::new(State::default()));
            let session = Session::with_backend(Mock(state.clone())).unwrap();
            let lease = session
                .acquire(
                    key().source,
                    SocketAddrV4::new(key().destination, 8443),
                    &route(),
                )
                .await
                .unwrap();
            state.lock().unwrap().deletes_always_fail = true;
            drop(lease);
            assert!(session.shutdown().await.is_err());
            assert!(session.shutdown().await.is_err());
        }

        #[test]
        fn allocator_avoids_existing_rule_priorities_and_handles_extended_tables() {
            let state = Arc::new(Mutex::new(State::default()));
            let existing = Policy::new(key(), 0x5e123456, 10_000);
            state.lock().unwrap().dump_rules.push(existing.rule);
            let (table, priority) = allocate(&mut Mock(state)).unwrap();
            assert_ne!(table, 0x5e123456);
            assert_eq!(priority, 10_001);
            assert!(attrs(&[3, 0, 1, 0]).is_err());
            assert!(attrs(&[8, 0, 1, 0, 1]).is_err());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(cidr: &str, interface: u32, gateway: Option<&str>) -> Route {
        let net: Ipv4Net = cidr.parse().unwrap();
        let mut route = Route::new(net.network().into(), net.prefix_len()).with_ifindex(interface);
        route.gateway = gateway.map(|ip| ip.parse().unwrap());
        route
    }

    #[test]
    fn default_gateway_cross_subnet_is_a_candidate_despite_more_specific_tun_route() {
        let gateway = route("0.0.0.0/0", 3, Some("192.168.187.1"));
        let tun = route("192.168.188.0/24", 99, None);
        assert_eq!(
            paths(
                "192.168.188.111".parse().unwrap(),
                &[gateway.clone(), tun],
                &[3]
            ),
            [gateway]
        );
    }

    #[test]
    fn chooses_longest_physical_prefix_and_caps_simultaneous_attempts() {
        let default = route("0.0.0.0/0", 3, Some("192.168.187.1"));
        let specific = route("192.168.188.0/24", 3, Some("192.168.187.2"));
        let mut table = vec![default, specific.clone()];
        table.extend((4..10).map(|index| route("0.0.0.0/0", index, Some("192.168.187.1"))));
        let selected = paths(
            "192.168.188.111".parse().unwrap(),
            &table,
            &(3..10).collect::<Vec<_>>(),
        );
        assert_eq!(selected.len(), 4);
        assert_eq!(selected[0], specific);
    }

    #[test]
    fn tunnel_subnet_and_non_unicast_targets_are_never_dialed() {
        let vpn: Ipv4Net = "10.8.0.0/24".parse().unwrap();
        for ip in [
            "0.0.0.0",
            "127.0.0.1",
            "224.0.0.1",
            "255.255.255.255",
            "169.254.1.2",
            "10.8.0.2",
        ] {
            assert!(!valid_destination(
                SocketAddrV4::new(ip.parse().unwrap(), 443),
                vpn
            ));
        }
        assert!(!valid_destination(
            SocketAddrV4::new("192.168.188.111".parse().unwrap(), 0),
            vpn
        ));
        assert!(valid_destination(
            SocketAddrV4::new("192.168.188.111".parse().unwrap(), 8443),
            vpn
        ));
    }

    #[tokio::test]
    async fn selecting_a_winner_synchronously_drops_losing_attempts() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        struct Guard(Arc<AtomicUsize>);
        impl Drop for Guard {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let released = Arc::new(AtomicUsize::new(0));
        let attempts = FuturesUnordered::new();
        for succeeds in [false, true, false] {
            let guard = Guard(released.clone());
            attempts.push(async move {
                let _guard = guard;
                if succeeds {
                    Ok(())
                } else {
                    std::future::pending::<io::Result<()>>().await
                }
            });
        }
        first_connected(attempts).await.unwrap();
        assert_eq!(released.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn timed_out_dial_synchronously_drops_every_attempt() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        struct Guard(Arc<AtomicUsize>);
        impl Drop for Guard {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let released = Arc::new(AtomicUsize::new(0));
        let attempts = FuturesUnordered::new();
        for _ in 0..4 {
            let guard = Guard(released.clone());
            attempts.push(async move {
                let _guard = guard;
                std::future::pending::<io::Result<()>>().await
            });
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(1), first_connected(attempts))
                .await
                .is_err()
        );
        assert_eq!(released.load(Ordering::SeqCst), 4);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn private_policy_tables_are_not_reused_as_physical_route_candidates() {
        let foreign = route("192.168.188.111/32", 3, None).with_table(100);
        assert!(paths("192.168.188.111".parse().unwrap(), &[foreign], &[3]).is_empty());
    }
}
