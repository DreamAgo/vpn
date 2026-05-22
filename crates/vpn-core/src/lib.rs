//! vpn-core: 共享业务核心
//!
//! 本 crate 仅定义 domain 模型与 trait（无 IO 依赖），
//! 具体实现由 vpn-server 等下游 crate 提供。
//!
//! 关键模块：
//! - [`error`]：业务错误类型 [`AppError`]
//! - [`time`]：可注入的 [`Clock`] trait（便于测试时间相关逻辑）
//! - [`service`]：业务服务 trait（PasswordHasher / TokenIssuer / IdGenerator）
//! - [`repository`]：数据仓库 trait（UserRepository 等，由后续 Story 填充）

pub mod error;
pub mod repository;
pub mod service;
pub mod time;

pub use error::{AppError, Result};
