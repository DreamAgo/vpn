//! Story 4.8: 跨平台 TUN 设备抽象。
//!
//! 设计：
//! - 统一基于 [`tun-rs`](https://docs.rs/tun-rs) 这一跨三平台 crate，按 `target_os`
//!   做差异化的接口命名约定与默认参数。
//! - [`TunDevice`] trait 暴露异步收发 + IP 配置 + 关闭，工厂函数 [`open_tun`]
//!   按平台分发。
//! - **可测纯逻辑**：[`Cidr`] 的解析与校验（IPv4/IPv6、前缀范围、掩码推导）。
//! - **需真机验证**：真实创建 utun/tun/WinTun 设备需要 root / CAP_NET_ADMIN /
//!   管理员权限，单测中以 `#[ignore]` 标注，结构留实现骨架。

use std::net::IpAddr;
use std::str::FromStr;

use async_trait::async_trait;

use crate::error::{PlatformError, Result};

/// 跨平台 TUN 设备 trait（L3，处理裸 IP 包）。
#[async_trait]
pub trait TunDevice: Send + Sync {
    /// 从设备读取一个 IP 包到 `buf`，返回写入字节数。
    async fn recv(&mut self, buf: &mut [u8]) -> Result<usize>;

    /// 向设备写入一个 IP 包，返回实际写入字节数。
    async fn send(&mut self, buf: &[u8]) -> Result<usize>;

    /// 为设备配置 IP 地址（CIDR，如 `10.0.0.2/24` 或 `fd00::2/64`）。
    async fn configure_ip(&mut self, cidr: &str) -> Result<()>;

    /// 关闭设备并释放资源。
    async fn close(&mut self) -> Result<()>;
}

/// 已解析、校验过的 CIDR（地址 + 前缀长度）。
///
/// 这是 [`TunDevice::configure_ip`] 的纯逻辑核心，便于在无设备的环境单测。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cidr {
    /// IP 地址部分。
    pub addr: IpAddr,
    /// 前缀长度（IPv4: 0..=32，IPv6: 0..=128）。
    pub prefix: u8,
}

impl Cidr {
    /// 是否为 IPv4。
    pub fn is_ipv4(&self) -> bool {
        self.addr.is_ipv4()
    }

    /// IPv4 的点分十进制子网掩码（仅当 `addr` 为 IPv4 时返回 `Some`）。
    ///
    /// 例如 prefix=24 → `255.255.255.0`。
    pub fn ipv4_netmask(&self) -> Option<std::net::Ipv4Addr> {
        match self.addr {
            IpAddr::V4(_) => {
                let bits: u32 = if self.prefix == 0 {
                    0
                } else {
                    u32::MAX
                        .checked_shl(32 - self.prefix as u32)
                        .unwrap_or(u32::MAX)
                };
                Some(std::net::Ipv4Addr::from(bits))
            }
            IpAddr::V6(_) => None,
        }
    }
}

impl FromStr for Cidr {
    type Err = PlatformError;

    fn from_str(s: &str) -> Result<Self> {
        let s = s.trim();
        let (addr_str, prefix_str) = s.split_once('/').ok_or_else(|| {
            PlatformError::InvalidArgument(format!(
                "CIDR `{s}` 缺少 `/前缀` 部分（期望如 10.0.0.2/24）"
            ))
        })?;

        let addr = IpAddr::from_str(addr_str.trim())
            .map_err(|e| PlatformError::InvalidArgument(format!("CIDR `{s}` 中的 IP 非法: {e}")))?;

        let prefix: u8 = prefix_str
            .trim()
            .parse()
            .map_err(|_| PlatformError::InvalidArgument(format!("CIDR `{s}` 中的前缀长度非法")))?;

        let max = if addr.is_ipv4() { 32 } else { 128 };
        if prefix > max {
            return Err(PlatformError::InvalidArgument(format!(
                "CIDR `{s}` 前缀 {prefix} 超出范围 0..={max}"
            )));
        }

        Ok(Cidr { addr, prefix })
    }
}

/// 按平台返回默认的 TUN 接口名。
///
/// - macOS 使用 `utun` 前缀（内核要求；具体序号由系统分配，名字仅作建议）。
/// - Linux / Windows 使用调用方给定名（如 `vpn-cli0`）。
pub fn normalize_interface_name(requested: &str) -> String {
    if cfg!(target_os = "macos") {
        // macOS utun 设备名必须形如 utunN；非该前缀时回退到 utun。
        if requested.starts_with("utun") {
            requested.to_string()
        } else {
            "utun".to_string()
        }
    } else {
        requested.to_string()
    }
}

/// 按当前平台打开一个 TUN 设备。
///
/// `name` 为期望的接口名（如 `vpn-cli0`；macOS 上会规整为 `utun*`）。
///
/// 真实创建设备需要相应权限（root / CAP_NET_ADMIN / 管理员），在缺乏权限或
/// 不支持的环境会返回 [`PlatformError`]，调用方应据此给出可读的提示。
pub fn open_tun(name: &str) -> Result<Box<dyn TunDevice>> {
    imp::open_tun(name)
}

// ===========================================================================
// 平台实现：统一基于 tun-rs。Linux/macOS 走相同的 unix 路径，Windows 同接口。
// ===========================================================================

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
mod imp {
    use super::{normalize_interface_name, Cidr, PlatformError, Result, TunDevice};
    use async_trait::async_trait;
    use std::str::FromStr;
    use tun_rs::AsyncDevice;

    /// 基于 tun-rs `AsyncDevice` 的统一 TUN 实现。
    pub(super) struct TunRsDevice {
        dev: AsyncDevice,
    }

    #[async_trait]
    impl TunDevice for TunRsDevice {
        async fn recv(&mut self, buf: &mut [u8]) -> Result<usize> {
            // tun-rs 的 recv/send 取 &self（内部用原子/异步原语），与 &mut self 兼容。
            Ok(self.dev.recv(buf).await?)
        }

        async fn send(&mut self, buf: &[u8]) -> Result<usize> {
            Ok(self.dev.send(buf).await?)
        }

        async fn configure_ip(&mut self, cidr: &str) -> Result<()> {
            let parsed = Cidr::from_str(cidr)?;
            match parsed.addr {
                std::net::IpAddr::V4(v4) => {
                    // set_network_address 接受 (ip, prefix, destination)。
                    self.dev
                        .set_network_address(v4, parsed.prefix, None)
                        .map_err(|e| PlatformError::Tun(format!("设置 IPv4 地址失败: {e}")))?;
                }
                std::net::IpAddr::V6(v6) => {
                    self.dev
                        .add_address_v6(v6, parsed.prefix)
                        .map_err(|e| PlatformError::Tun(format!("设置 IPv6 地址失败: {e}")))?;
                }
            }
            Ok(())
        }

        async fn close(&mut self) -> Result<()> {
            // tun-rs 在 Drop 时释放底层 fd / 句柄（实际关闭设备）；
            // 这里先把链路置为 down，使其停止收发，跨三平台均可用。
            // （`shutdown()` 仅在 Windows / unix+experimental 可用，此处不依赖。）
            self.dev
                .enabled(false)
                .map_err(|e| PlatformError::Tun(format!("关闭设备失败: {e}")))?;
            Ok(())
        }
    }

    pub(super) fn open_tun(name: &str) -> Result<Box<dyn TunDevice>> {
        let iface = normalize_interface_name(name);
        let dev = tun_rs::DeviceBuilder::new()
            .name(iface)
            .mtu(1400)
            .build_async()
            .map_err(|e| {
                PlatformError::Tun(format!(
                    "创建 TUN 设备失败（通常需要 root/CAP_NET_ADMIN/管理员权限）: {e}"
                ))
            })?;
        Ok(Box::new(TunRsDevice { dev }))
    }
}

// 其它操作系统（如 *BSD）：编译期不报错，运行期返回 Unsupported。
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
mod imp {
    use super::{PlatformError, Result, TunDevice};

    pub(super) fn open_tun(_name: &str) -> Result<Box<dyn TunDevice>> {
        Err(PlatformError::Unsupported(
            "TUN 设备仅支持 linux/macos/windows".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn parse_ipv4_cidr() {
        let c: Cidr = "10.0.0.2/24".parse().unwrap();
        assert_eq!(c.addr, IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)));
        assert_eq!(c.prefix, 24);
        assert!(c.is_ipv4());
    }

    #[test]
    fn parse_ipv6_cidr() {
        let c: Cidr = "fd00::2/64".parse().unwrap();
        assert_eq!(
            c.addr,
            IpAddr::V6(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 2))
        );
        assert_eq!(c.prefix, 64);
        assert!(!c.is_ipv4());
    }

    #[test]
    fn parse_trims_whitespace() {
        let c: Cidr = "  192.168.1.1/16  ".parse().unwrap();
        assert_eq!(c.prefix, 16);
    }

    #[test]
    fn reject_missing_prefix() {
        assert!("10.0.0.2".parse::<Cidr>().is_err());
    }

    #[test]
    fn reject_bad_ip() {
        assert!("999.0.0.1/24".parse::<Cidr>().is_err());
        assert!("not-an-ip/24".parse::<Cidr>().is_err());
    }

    #[test]
    fn reject_prefix_out_of_range() {
        assert!("10.0.0.2/33".parse::<Cidr>().is_err()); // v4 上限 32
        assert!("fd00::2/129".parse::<Cidr>().is_err()); // v6 上限 128
    }

    #[test]
    fn reject_non_numeric_prefix() {
        assert!("10.0.0.2/abc".parse::<Cidr>().is_err());
    }

    #[test]
    fn ipv4_netmask_derivation() {
        let c: Cidr = "10.0.0.2/24".parse().unwrap();
        assert_eq!(c.ipv4_netmask(), Some(Ipv4Addr::new(255, 255, 255, 0)));

        let c: Cidr = "10.0.0.2/16".parse().unwrap();
        assert_eq!(c.ipv4_netmask(), Some(Ipv4Addr::new(255, 255, 0, 0)));

        let c: Cidr = "10.0.0.2/0".parse().unwrap();
        assert_eq!(c.ipv4_netmask(), Some(Ipv4Addr::new(0, 0, 0, 0)));

        let c: Cidr = "10.0.0.2/32".parse().unwrap();
        assert_eq!(c.ipv4_netmask(), Some(Ipv4Addr::new(255, 255, 255, 255)));
    }

    #[test]
    fn ipv6_has_no_v4_netmask() {
        let c: Cidr = "fd00::1/64".parse().unwrap();
        assert_eq!(c.ipv4_netmask(), None);
    }

    #[test]
    fn interface_name_normalization() {
        // 平台相关：仅断言不 panic 且返回非空。
        assert!(!normalize_interface_name("vpn-cli0").is_empty());
        if cfg!(target_os = "macos") {
            assert_eq!(normalize_interface_name("vpn-cli0"), "utun");
            assert_eq!(normalize_interface_name("utun9"), "utun9");
        } else {
            assert_eq!(normalize_interface_name("vpn-cli0"), "vpn-cli0");
        }
    }

    #[tokio::test]
    #[ignore = "真机验证：需要 root/CAP_NET_ADMIN/管理员权限创建真实 TUN 设备"]
    async fn open_real_device() {
        let dev = open_tun("vpn-cli0");
        assert!(dev.is_ok(), "open_tun 失败: {:?}", dev.err());
    }
}
