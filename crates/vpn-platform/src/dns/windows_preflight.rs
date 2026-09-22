//! Read-only fast paths. Only a conclusive empty result may skip cleanup.
//! Never cache these snapshots: a previous connection can leave new state behind.
use super::CleanupCheck;
use std::{io, ptr};
use windows_sys::Win32::{
    Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_NO_DATA, NO_ERROR},
    NetworkManagement::IpHelper::{
        GetAdaptersAddresses, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_MULTICAST,
        GAA_FLAG_SKIP_UNICAST, IP_ADAPTER_ADDRESSES_LH,
    },
    Networking::WinSock::AF_UNSPEC,
};
use winreg::{enums::HKEY_LOCAL_MACHINE, RegKey};

const POLICY_PATH: &str = r"SYSTEM\CurrentControlSet\Services\Dnscache\Parameters\DnsPolicyConfig";

pub(super) fn needed(check: CleanupCheck) -> io::Result<bool> {
    match check {
        CleanupCheck::MacosDynamicStore => Ok(true),
        CleanupCheck::Policy => policy_present(&RegKey::predef(HKEY_LOCAL_MACHINE), POLICY_PATH),
        CleanupCheck::Interface(index) => interface_dns_present(index),
    }
}

fn policy_present(root: &RegKey, path: &str) -> io::Result<bool> {
    match root.open_subkey(path) {
        Ok(key) => Ok(key.query_info()?.sub_keys != 0),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn interface_dns_present(index: u32) -> io::Result<bool> {
    let mut size = 15 * 1024u32;
    for _ in 0..3 {
        // Use storage aligned for IP_ADAPTER_ADDRESSES_LH (including its u64 unions).
        let count = (size as usize).div_ceil(std::mem::size_of::<IP_ADAPTER_ADDRESSES_LH>());
        let mut storage = vec![std::mem::MaybeUninit::<IP_ADAPTER_ADDRESSES_LH>::uninit(); count];
        let first = storage.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        // SAFETY: storage is writable, suitably aligned and at least `size` bytes.
        // No pointers from the API escape the lifetime of storage.
        let status = unsafe {
            GetAdaptersAddresses(
                AF_UNSPEC as u32,
                GAA_FLAG_SKIP_UNICAST | GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST,
                ptr::null(),
                first,
                &mut size,
            )
        };
        match status {
            ERROR_BUFFER_OVERFLOW => continue,
            // An absent adapter is not evidence that the requested adapter has no DNS.
            ERROR_NO_DATA => return Ok(true),
            NO_ERROR => {
                // SAFETY: successful API call initialized this list in live storage.
                return Ok(unsafe { dns_in_snapshot(first, index) }.unwrap_or(true));
            }
            error => return Err(io::Error::from_raw_os_error(error as i32)),
        }
    }
    Err(io::Error::other("网络接口列表持续变化，无法确定 DNS 状态"))
}

/// The caller must keep the complete API-initialized list alive for this call.
unsafe fn dns_in_snapshot(mut current: *const IP_ADAPTER_ADDRESSES_LH, index: u32) -> Option<bool> {
    while !current.is_null() {
        // SAFETY: guaranteed by the caller; no pointer escapes this traversal.
        let adapter = unsafe { &*current };
        if unsafe { adapter.Anonymous1.Anonymous.IfIndex } == index {
            return Some(!adapter.FirstDnsServerAddress.is_null());
        }
        current = adapter.Next;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use winreg::enums::HKEY_CURRENT_USER;

    #[test]
    fn registry_preflight_skips_only_missing_or_empty_store() {
        // Isolated HKCU fixture: never writes real NRPT state or needs elevation.
        let root = RegKey::predef(HKEY_CURRENT_USER);
        let path = format!(r"Software\YilianDnsPreflightTest{}", std::process::id());
        root.delete_subkey_all(&path).ok();
        assert!(!policy_present(&root, &path).unwrap());
        let (key, _) = root.create_subkey(&path).unwrap();
        assert!(!policy_present(&root, &path).unwrap());
        let (rule, _) = key.create_subkey("rule").unwrap();
        assert!(policy_present(&root, &path).unwrap());
        drop(rule);
        key.delete_subkey("rule").unwrap();
        assert!(!policy_present(&root, &path).unwrap());
        drop(key);
        root.delete_subkey(&path).unwrap();
    }

    #[test]
    fn snapshot_matches_only_requested_interface_and_keeps_any_dns_family() {
        // Zero is valid for these C structs; only indexes, links and DNS pointers are read.
        let mut other: IP_ADAPTER_ADDRESSES_LH = unsafe { std::mem::zeroed() };
        let mut target: IP_ADAPTER_ADDRESSES_LH = unsafe { std::mem::zeroed() };
        other.Anonymous1.Anonymous.IfIndex = 10;
        target.Anonymous1.Anonymous.IfIndex = 20;
        other.Next = &mut target;
        // A non-null marker is sufficient; dns_in_snapshot never dereferences DNS nodes.
        other.FirstDnsServerAddress = std::ptr::NonNull::dangling().as_ptr();
        assert_eq!(unsafe { dns_in_snapshot(&other, 20) }, Some(false));
        assert_eq!(unsafe { dns_in_snapshot(&other, 10) }, Some(true));
        assert_eq!(unsafe { dns_in_snapshot(&other, 30) }, None);
        target.FirstDnsServerAddress = std::ptr::NonNull::dangling().as_ptr();
        assert_eq!(unsafe { dns_in_snapshot(&target, 20) }, Some(true));
    }

    #[test]
    fn unknown_interface_does_not_skip_cleanup() {
        assert!(interface_dns_present(u32::MAX).unwrap());
    }
}
