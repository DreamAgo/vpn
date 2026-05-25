//! 共享应用状态（注入到所有 handler）。

use std::sync::Arc;
use vpn_core::time::{Clock, SystemClock};

use crate::services::AuthService;

/// AppState 持有所有跨 handler 共享的资源。
///
/// 增加 service / repository 时在此结构追加字段。
#[derive(Clone)]
pub struct AppState {
    pub clock: Arc<dyn Clock>,
    pub auth_service: Option<Arc<AuthService>>,
}

impl AppState {
    /// 用最小依赖构造 AppState（仅用于 Story 1.x 的 health handler 测试）。
    pub fn new() -> Self {
        Self {
            clock: Arc::new(SystemClock),
            auth_service: None,
        }
    }

    pub fn with_auth_service(mut self, svc: Arc<AuthService>) -> Self {
        self.auth_service = Some(svc);
        self
    }

    /// 获取 AuthService，未初始化则返回错误（启动顺序问题）。
    pub fn auth_service(&self) -> Result<Arc<AuthService>, vpn_core::AppError> {
        self.auth_service
            .clone()
            .ok_or_else(|| vpn_core::AppError::Config("auth_service 未初始化".to_string()))
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
