//! vpn-wireguard: WireGuard 数据平面封装。
//!
//! 模块：
//! - [`keys`]：Curve25519 密钥对生成与公钥推导
//! - [`ip_pool`]：VPN 虚拟 IP 静态分配池（纯逻辑，可测）
//! - [`config`]：peer 配置类型 + 客户端 .conf 渲染
//! - [`control`]：[`control::WireGuardControl`] 控制平面抽象 + 测试用 Noop 实现
//!
//! 设计原则：把「可测的纯逻辑」（密钥、IP 池、配置渲染）与「需 root 的系统集成」
//! （真实 boringtun 隧道 / TUN / UDP）分离，后者隔离在 [`control::WireGuardControl`]
//! trait 之后，便于服务端业务层用 Noop 实现进行完整单元/集成测试。

pub mod config;
pub mod control;
pub mod ip_pool;
pub mod kernel;
pub mod keys;

pub use config::{render_client_config, WgPeerConfig};
pub use control::{NoopWireGuardControl, WireGuardControl};
pub use ip_pool::IpPool;
pub use kernel::KernelWireGuardControl;
pub use keys::{generate_keypair, public_key_from_private, WgKeypair};
