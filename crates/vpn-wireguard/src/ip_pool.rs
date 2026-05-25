//! VPN 虚拟 IP 静态分配池。
//!
//! 设计：
//! - 池绑定一个 IPv4 子网（如 10.8.0.0/24）。
//! - 保留网络地址（.0）、服务端地址（.1）、广播地址（.255）。
//! - `allocate()` 返回下一个空闲 IP；池耗尽返回 [`AppError::IpPoolExhausted`]。
//! - 「同一 user 重连得到同一 IP」由调用方（peer_service）借助 peers 表实现：
//!   先查库复用，库中无则 `allocate()`；启动时用 `mark_used()` 回填已占用 IP。
//!
//! 本结构只做「下一个空闲地址」的纯逻辑，便于单测；不持有任何 IO。

use std::collections::BTreeSet;
use std::net::Ipv4Addr;

use ipnet::Ipv4Net;
use vpn_core::{AppError, Result};

/// 静态 IP 分配池（内存态，启动时由 DB 回填）。
#[derive(Debug, Clone)]
pub struct IpPool {
    subnet: Ipv4Net,
    /// 保留地址（网络/服务端/广播），不参与分配。
    reserved: BTreeSet<Ipv4Addr>,
    /// 已分配地址。
    allocated: BTreeSet<Ipv4Addr>,
}

impl IpPool {
    /// 用子网创建池。自动保留网络地址、服务端地址（首个可用，即 .1）、广播地址。
    pub fn new(subnet: Ipv4Net) -> Self {
        let mut reserved = BTreeSet::new();
        reserved.insert(subnet.network());
        reserved.insert(subnet.broadcast());
        // 服务端取子网首个主机地址（通常 .1）
        if let Some(server) = subnet.hosts().next() {
            reserved.insert(server);
        }
        Self {
            subnet,
            reserved,
            allocated: BTreeSet::new(),
        }
    }

    /// 服务端地址（子网首个主机地址）。
    pub fn server_addr(&self) -> Option<Ipv4Addr> {
        self.subnet.hosts().next()
    }

    /// 子网（CIDR）。
    pub fn subnet(&self) -> Ipv4Net {
        self.subnet
    }

    /// 启动时回填已占用 IP（来自 peers 表）。非本子网内地址忽略。
    pub fn mark_used(&mut self, ip: Ipv4Addr) {
        if self.subnet.contains(&ip) && !self.reserved.contains(&ip) {
            self.allocated.insert(ip);
        }
    }

    /// 分配下一个空闲 IP。池耗尽返回 [`AppError::IpPoolExhausted`]。
    pub fn allocate(&mut self) -> Result<Ipv4Addr> {
        for host in self.subnet.hosts() {
            if !self.reserved.contains(&host) && !self.allocated.contains(&host) {
                self.allocated.insert(host);
                return Ok(host);
            }
        }
        Err(AppError::IpPoolExhausted)
    }

    /// 释放 IP，使其可被再次分配。
    pub fn release(&mut self, ip: Ipv4Addr) {
        self.allocated.remove(&ip);
    }

    /// 当前已分配数量。
    pub fn allocated_count(&self) -> usize {
        self.allocated.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool_24() -> IpPool {
        IpPool::new("10.8.0.0/24".parse().unwrap())
    }

    #[test]
    fn reserves_network_server_broadcast() {
        let pool = pool_24();
        assert_eq!(pool.server_addr(), Some("10.8.0.1".parse().unwrap()));
        // .0 / .1 / .255 都被保留
        assert!(pool.reserved.contains(&"10.8.0.0".parse().unwrap()));
        assert!(pool.reserved.contains(&"10.8.0.1".parse().unwrap()));
        assert!(pool.reserved.contains(&"10.8.0.255".parse().unwrap()));
    }

    #[test]
    fn allocates_from_dot_two() {
        let mut pool = pool_24();
        let ip = pool.allocate().unwrap();
        assert_eq!(ip, "10.8.0.2".parse::<Ipv4Addr>().unwrap());
        let ip2 = pool.allocate().unwrap();
        assert_eq!(ip2, "10.8.0.3".parse::<Ipv4Addr>().unwrap());
    }

    #[test]
    fn release_makes_ip_reusable() {
        let mut pool = pool_24();
        let ip = pool.allocate().unwrap();
        pool.release(ip);
        let ip2 = pool.allocate().unwrap();
        assert_eq!(ip, ip2);
    }

    #[test]
    fn mark_used_skips_allocated() {
        let mut pool = pool_24();
        pool.mark_used("10.8.0.2".parse().unwrap());
        let ip = pool.allocate().unwrap();
        assert_eq!(ip, "10.8.0.3".parse::<Ipv4Addr>().unwrap());
    }

    #[test]
    fn exhaustion_returns_error() {
        // /30 子网：4 个地址，保留 .0/.1/.3，仅 .2 可分配
        let mut pool = IpPool::new("10.9.0.0/30".parse().unwrap());
        let _ = pool.allocate().unwrap(); // .2
        assert!(matches!(pool.allocate(), Err(AppError::IpPoolExhausted)));
    }

    #[test]
    fn full_24_pool_allocates_252_hosts() {
        let mut pool = pool_24();
        let mut count = 0;
        while pool.allocate().is_ok() {
            count += 1;
        }
        // 256 - 网络 - 服务端 - 广播 = 253；hosts() 已排除网络与广播 => 254，再减服务端 .1 = 253
        assert_eq!(count, 253);
        assert_eq!(pool.allocated_count(), 253);
    }
}
