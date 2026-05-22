//! Tower 中间件集合。
//!
//! Story 1.5 + 1.6 当前包含：
//! - request_id（tower-http 内置，在 app.rs 配置）
//! - HTTPS 强制（[`https_redirect`]）
//!
//! 后续 Story 添加：auth / audit / rate-limit。

pub mod https_redirect;
