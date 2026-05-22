//! 时间抽象，便于测试时间相关逻辑（如 Token 过期、心跳超时）。

use chrono::{DateTime, Utc};

/// 时钟 trait，生产代码注入 [`SystemClock`]，测试代码注入 mock。
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;

    /// 当前时间的 unix milliseconds（用于 API 响应 timestamp 字段）。
    fn now_unix_ms(&self) -> i64 {
        self.now().timestamp_millis()
    }
}

/// 系统时钟实现（生产用）。
#[derive(Debug, Clone, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_clock_returns_current_time() {
        let clock = SystemClock;
        let t1 = clock.now();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let t2 = clock.now();
        assert!(t2 > t1);
    }

    #[test]
    fn unix_ms_is_positive() {
        let clock = SystemClock;
        assert!(clock.now_unix_ms() > 0);
    }
}
