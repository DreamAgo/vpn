//! vpn-api-types: 前后端共享 DTO
//!
//! 核心契约：
//! - 所有 API 响应都包裹在 [`ApiResponse`] 信封中
//! - 列表数据使用 [`Page`] 分页结构
//! - 业务错误码定义在 [`error_codes`] 模块
//!
//! 本 crate 不依赖任何 IO crate（无 sqlx / axum / reqwest），仅 serde。

pub mod audit;
pub mod auth;
pub mod envelope;
pub mod error_codes;
pub mod group;
pub mod peer;
pub mod subnet;
pub mod system;
pub mod user;

pub use envelope::{ApiResponse, Page};
