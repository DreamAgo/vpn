//! vpn-cli: VPN 客户端命令行 + 后台 daemon。
//!
//! 模块划分（Epic 4 客户端 CLI）：
//! - [`cli`]（4.13 / 4.15）：clap 命令解析、login/logout、up/down/status 控制命令、
//!   daemon 服务管理。
//! - [`api`]（4.12）：reqwest API 客户端，统一 [`vpn_api_types::ApiResponse`] 信封
//!   解包，token 过期自动刷新重试。
//! - [`ipc`]（4.11）：CLI <-> daemon 本地 IPC（unix socket + 行 JSON 协议）。
//! - [`daemon`]（4.14）：daemon 主循环、连接状态机、心跳、数据面转发骨架。
//! - [`reconnect`]（4.16）：指数退避 + 抖动重连（纯函数，可测）。
//! - [`netmon`]（4.17）：网络变化检测（trait + 轮询降级实现）。
//! - [`config`]：凭证持久化（vpn-platform CredentialStore）与 daemon 配置。
//! - [`error`]：统一错误类型。
//!
//! 设计原则：把命令解析、信封解包、IPC 编解码、退避算法、网络快照比较等
//! **纯逻辑**与需要真机（设备 / root / 网络 / 钥匙串）的系统集成路径分离，
//! 前者全量单测，后者保留可编译、不 panic 的骨架并标注「真机验证」。

pub mod api;
pub mod cli;
pub mod config;
pub mod daemon;
pub mod error;
pub mod ipc;
pub mod netmon;
pub mod reconnect;
pub mod wg_userspace;
