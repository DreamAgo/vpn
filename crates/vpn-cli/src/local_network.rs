//! Read active physical IPv4 addresses for configured local-network rules.
//!
//! Only positively identified hardware interfaces with a working link can match
//! a rule. This module neither probes destinations nor reads or modifies routes.

#[cfg(any(unix, target_os = "windows"))]
use std::collections::BTreeMap;
use std::{io, net::Ipv4Addr};

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

/// Read physical interface addresses directly: a routing lookup may report the
/// address of another VPN, which must never activate a local-network rule.
#[cfg(unix)]
pub(crate) fn physical_ipv4_addresses(tun_index: u32) -> io::Result<Vec<Ipv4Addr>> {
    let interfaces = InterfaceAddresses::new()?;
    let mut next = interfaces.0;
    let mut physical = BTreeMap::new();
    let mut addresses = Vec::new();
    while !next.is_null() {
        // SAFETY: getifaddrs owns this list for the lifetime of interfaces.
        let entry = unsafe { &*next };
        next = entry.ifa_next;
        if entry.ifa_addr.is_null()
            || entry.ifa_name.is_null()
            || unsafe { (*entry.ifa_addr).sa_family } as i32 != libc::AF_INET
            || entry.ifa_flags & libc::IFF_UP as u32 == 0
            || entry.ifa_flags & (libc::IFF_LOOPBACK | libc::IFF_POINTOPOINT) as u32 != 0
        {
            continue;
        }
        // SAFETY: getifaddrs provides a NUL-terminated interface name.
        let index = unsafe { libc::if_nametoindex(entry.ifa_name) };
        if index == 0
            || index == tun_index
            || !*physical
                .entry(index)
                .or_insert_with(|| interface_is_physical(index) && interface_link_is_active(index))
        {
            continue;
        }
        // SAFETY: the family check above establishes sockaddr_in layout.
        let address = unsafe { &*entry.ifa_addr.cast::<libc::sockaddr_in>() };
        let ip = Ipv4Addr::from(address.sin_addr.s_addr.to_ne_bytes());
        if usable_local_address(ip) {
            addresses.push(ip);
        }
    }
    addresses.sort_unstable();
    addresses.dedup();
    Ok(addresses)
}

// Declarative route bypass requires positive link-state evidence. A
// disconnected adapter may remain administratively UP with a static address;
// that old address must not keep VPN routes excluded after switching networks.
#[cfg(target_os = "linux")]
fn interface_link_is_active(ifindex: u32) -> bool {
    let Ok(name) = interface_name(ifindex) else {
        return false;
    };
    let Ok(name) = name.to_str() else {
        return false;
    };
    let base = std::path::Path::new("/sys/class/net").join(name);
    let (Ok(carrier), Ok(operstate)) = (
        std::fs::read_to_string(base.join("carrier")),
        std::fs::read_to_string(base.join("operstate")),
    ) else {
        return false;
    };
    linux_link_is_active(&carrier, &operstate)
}

#[cfg(any(target_os = "linux", test))]
fn linux_link_is_active(carrier: &str, operstate: &str) -> bool {
    carrier.trim() == "1" && operstate.trim() == "up"
}

// Darwin net/if.h declares ifmediareq under #pragma pack(4), including the
// trailing pointer. Using ordinary repr(C) would generate the wrong ioctl size.
#[cfg(target_os = "macos")]
#[repr(C, packed(4))]
struct MacIfMediaReq {
    name: [libc::c_char; libc::IF_NAMESIZE],
    current: libc::c_int,
    mask: libc::c_int,
    status: libc::c_int,
    active: libc::c_int,
    count: libc::c_int,
    list: *mut libc::c_int,
}

#[cfg(target_os = "macos")]
fn interface_link_is_active(ifindex: u32) -> bool {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    let Ok(name) = interface_name(ifindex) else {
        return false;
    };
    // This unconnected control socket only carries the read-only ioctl below.
    // No destination is contacted and no socket is kept between observations.
    // SAFETY: these are valid socket constants and no pointers are passed.
    let descriptor = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) };
    if descriptor < 0 {
        return false;
    }
    // SAFETY: socket returned a new valid descriptor, uniquely owned here.
    let socket = unsafe { OwnedFd::from_raw_fd(descriptor) };
    let mut request = MacIfMediaReq {
        name: [0; libc::IF_NAMESIZE],
        current: 0,
        mask: 0,
        status: 0,
        active: 0,
        count: 0,
        list: std::ptr::null_mut(),
    };
    let name = name.as_bytes_with_nul();
    if name.len() > request.name.len() {
        return false;
    }
    for (target, &byte) in request.name.iter_mut().zip(name) {
        *target = byte as libc::c_char;
    }
    // Darwin sys/sockio.h: _IOWR('i', 56, struct ifmediareq). A zero count and
    // null list request only status, with no second media-list allocation.
    const SIOCGIFMEDIA: libc::c_ulong = 0xc000_0000
        | ((std::mem::size_of::<MacIfMediaReq>() as libc::c_ulong & 0x1fff) << 16)
        | ((b'i' as libc::c_ulong) << 8)
        | 56;
    // SAFETY: socket is live; request has the SDK's packed ABI and a bounded,
    // NUL-terminated name. This read-only ioctl writes only into request.
    let result = unsafe { libc::ioctl(socket.as_raw_fd(), SIOCGIFMEDIA, &mut request) };
    result == 0 && macos_media_is_active(request.status)
}

#[cfg(any(target_os = "macos", test))]
fn macos_media_is_active(status: i32) -> bool {
    // net/if_media.h: IFM_AVALID says IFM_ACTIVE is meaningful; IFM_ACTIVE says
    // the interface is attached to a working network. IFF_RUNNING is weaker.
    status & 0x3 == 0x3
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn interface_link_is_active(_ifindex: u32) -> bool {
    false
}

#[cfg(target_os = "windows")]
pub(crate) fn physical_ipv4_addresses(tun_index: u32) -> io::Result<Vec<Ipv4Addr>> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        FreeMibTable, GetUnicastIpAddressTable, MIB_UNICASTIPADDRESS_TABLE,
    };
    use windows_sys::Win32::Networking::WinSock::{IpDadStatePreferred, AF_INET};

    struct AddressTable(*mut MIB_UNICASTIPADDRESS_TABLE);
    impl Drop for AddressTable {
        fn drop(&mut self) {
            // SAFETY: this is the allocation returned by GetUnicastIpAddressTable.
            unsafe { FreeMibTable(self.0.cast()) };
        }
    }
    let mut table = std::ptr::null_mut();
    // SAFETY: table is a valid output pointer; successful allocation is owned below.
    let result = unsafe { GetUnicastIpAddressTable(AF_INET, &mut table) };
    if result != 0 {
        return Err(io::Error::from_raw_os_error(result as i32));
    }
    if table.is_null() {
        return Err(io::Error::other("physical IPv4 address table unavailable"));
    }
    let table = AddressTable(table);
    // SAFETY: the API allocates NumEntries rows in the variable-length Table field.
    let rows = unsafe {
        std::slice::from_raw_parts((*table.0).Table.as_ptr(), (*table.0).NumEntries as usize)
    };
    let mut physical = BTreeMap::new();
    let mut addresses = Vec::new();
    for row in rows {
        if row.InterfaceIndex == tun_index
            || row.DadState != IpDadStatePreferred
            || !*physical
                .entry(row.InterfaceIndex)
                .or_insert_with(|| interface_is_physical(row.InterfaceIndex))
        {
            continue;
        }
        // SAFETY: the table was requested for AF_INET; verify before union access.
        if unsafe { row.Address.si_family } != AF_INET {
            continue;
        }
        let ip = Ipv4Addr::from(unsafe { row.Address.Ipv4.sin_addr.S_un.S_addr }.to_ne_bytes());
        if usable_local_address(ip) {
            addresses.push(ip);
        }
    }
    addresses.sort_unstable();
    addresses.dedup();
    Ok(addresses)
}

#[cfg(not(any(unix, target_os = "windows")))]
pub(crate) fn physical_ipv4_addresses(_tun_index: u32) -> io::Result<Vec<Ipv4Addr>> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "physical network detection unavailable",
    ))
}

fn usable_local_address(address: Ipv4Addr) -> bool {
    !address.is_unspecified()
        && !address.is_loopback()
        && !address.is_link_local()
        && !address.is_multicast()
        && !address.is_broadcast()
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

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn interface_is_physical(_ifindex: u32) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_unicast_addresses_cannot_activate_a_local_network_rule() {
        for address in [
            "0.0.0.0",
            "127.0.0.1",
            "127.3.4.5",
            "169.254.12.3",
            "224.0.0.1",
            "255.255.255.255",
        ] {
            assert!(!usable_local_address(address.parse().unwrap()), "{address}");
        }
        // Matching is configured by CIDR, not restricted to fixed RFC1918 lists.
        for address in ["192.168.187.2", "10.1.2.3", "172.0.0.2", "203.0.113.2"] {
            assert!(usable_local_address(address.parse().unwrap()), "{address}");
        }
    }

    #[test]
    fn bypass_requires_a_working_link_even_when_adapter_keeps_its_address() {
        // A disconnected, administratively UP adapter can retain an address;
        // unknown/dormant/incomplete link reports must restore VPN coverage.
        assert!(linux_link_is_active("1\n", "up\n"));
        for state in ["down", "dormant", "unknown", "lowerlayerdown", ""] {
            assert!(!linux_link_is_active("1", state), "{state}");
        }
        for carrier in ["0", "", "invalid"] {
            assert!(!linux_link_is_active(carrier, "up"));
        }
        assert!(macos_media_is_active(0x3));
        assert!(macos_media_is_active(0x103));
        for status in [0, 0x1, 0x2, 0x100] {
            assert!(!macos_media_is_active(status), "{status}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn darwin_media_request_matches_the_packed_sdk_abi() {
        assert_eq!(std::mem::align_of::<MacIfMediaReq>(), 4);
        assert_eq!(std::mem::offset_of!(MacIfMediaReq, status), 24);
        assert_eq!(std::mem::offset_of!(MacIfMediaReq, list), 36);
        assert_eq!(std::mem::size_of::<MacIfMediaReq>(), 44);
    }
}
