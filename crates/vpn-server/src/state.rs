//! 共享应用状态（注入到所有 handler）。

use std::sync::Arc;
use vpn_core::time::{Clock, SystemClock};

/// AppState 持有所有跨 handler 共享的资源。
///
/// 业务 service / repository 由后续 Story 添加到此结构。
#[derive(Clone)]
pub struct AppState {
    pub clock: Arc<dyn Clock>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            clock: Arc::new(SystemClock),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
