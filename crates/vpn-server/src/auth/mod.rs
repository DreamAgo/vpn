//! 认证子系统：JWT claims 解析、CurrentUser extractor、RequireAdmin extractor。
//!
//! Story 2.7 实现。

pub mod extractor;

pub use extractor::{CurrentUser, RequireAdmin};
