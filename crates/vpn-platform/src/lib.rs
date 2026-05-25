//! vpn-platform: 客户端跨平台抽象层。
//!
//! 本 crate 把 VPN 客户端对操作系统的依赖收敛到少量 trait 后，按平台提供实现：
//!
//! - [`tun`]（Story 4.8）：[`TunDevice`] —— 跨 Linux/macOS/Windows 的 TUN 设备
//!   收发与 IP 配置，基于 `tun-rs`。
//! - [`credential`]（Story 4.9）：[`CredentialStore`] —— 系统钥匙串
//!   （[`KeyringCredentialStore`]）为主路径，加密文件
//!   （[`FileCredentialStore`]）为降级 / 可测路径。
//! - [`daemon`]（Story 4.10）：[`DaemonRuntime`] —— systemd user /
//!   launchd LaunchAgent / Windows Service 服务生命周期管理。
//! - [`error`]：本 crate 自有错误类型 [`PlatformError`] 与 [`Result`] 别名
//!   （不耦合 vpn-core）。
//!
//! 设计约束：需要真实硬件 / root / 桌面会话的系统集成路径在当前 CI 主机无法
//! 验证，相关函数保留可编译的实现骨架并返回结构化错误，纯逻辑（CIDR 解析、
//! 凭证文件加解密、服务文件渲染与路径推导）均带单测。

pub mod credential;
pub mod daemon;
pub mod error;
pub mod tun;

// === 主要类型 re-export ===
pub use error::{PlatformError, Result};

pub use tun::{open_tun, Cidr, TunDevice};

pub use credential::{
    CredentialStore, FileCredentialStore, KeyringCredentialStore, DEFAULT_SERVICE,
};

pub use daemon::{
    default_runtime, render_launchd_plist, render_systemd_unit, DaemonRuntime, DaemonStatus,
    DEFAULT_LAUNCHD_LABEL, DEFAULT_SERVICE_NAME,
};
