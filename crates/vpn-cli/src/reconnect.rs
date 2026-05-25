//! Story 4.16: 自动重连退避策略。
//!
//! 指数退避 + 抖动（jitter），用于隧道异常 / 心跳失败后避免「重连风暴」。
//!
//! 核心是纯函数 [`BackoffPolicy::delay_for_attempt`]（给定第 N 次尝试返回基准
//! 退避时长）与 [`BackoffPolicy::next_delay`]（含抖动），**完全可单测**且不依赖
//! 时钟 / 随机源（抖动通过显式传入的 `[0,1)` 随机数计算，便于确定性测试）。

use std::time::Duration;

/// 指数退避配置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackoffPolicy {
    /// 初始退避（第 0 次重连）。
    pub base: Duration,
    /// 退避上限。
    pub max: Duration,
    /// 倍增因子（通常为 2）。
    pub factor: u32,
    /// 抖动比例（0..=100，百分比）。例如 20 表示在基准值上 ±20% 抖动。
    pub jitter_pct: u8,
}

impl Default for BackoffPolicy {
    fn default() -> Self {
        Self {
            base: Duration::from_secs(1),
            max: Duration::from_secs(60),
            factor: 2,
            jitter_pct: 20,
        }
    }
}

impl BackoffPolicy {
    /// 第 `attempt`（从 0 开始）次重连的**基准**退避（未加抖动），裁剪到 `max`。
    ///
    /// `attempt=0 -> base`，`attempt=1 -> base*factor`，……，超过 `max` 后恒为 `max`。
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        let base_ms = self.base.as_millis() as u64;
        let max_ms = self.max.as_millis() as u64;
        // 用饱和乘法避免溢出：base * factor^attempt。
        let mut ms = base_ms;
        for _ in 0..attempt {
            ms = ms.saturating_mul(self.factor as u64);
            if ms >= max_ms {
                return self.max;
            }
        }
        Duration::from_millis(ms.min(max_ms))
    }

    /// 在基准退避基础上施加抖动。
    ///
    /// `rand_unit` 必须落在 `[0.0, 1.0)`，用于把抖动映射到
    /// `[-jitter_pct%, +jitter_pct%]`。结果被裁剪到 `[0, max]`。
    ///
    /// 分离随机源使该函数纯净、可确定性单测。
    pub fn next_delay(&self, attempt: u32, rand_unit: f64) -> Duration {
        let base = self.delay_for_attempt(attempt);
        if self.jitter_pct == 0 {
            return base;
        }
        let base_ms = base.as_millis() as f64;
        let jitter_frac = (self.jitter_pct as f64) / 100.0;
        // rand_unit ∈ [0,1) → 偏移系数 ∈ [-jitter_frac, +jitter_frac)
        let r = rand_unit.clamp(0.0, 1.0);
        let offset = (r * 2.0 - 1.0) * jitter_frac;
        let jittered = (base_ms * (1.0 + offset)).max(0.0);
        let max_ms = self.max.as_millis() as f64;
        Duration::from_millis(jittered.min(max_ms) as u64)
    }
}

/// 退避状态机：跟踪当前尝试次数，便于在重连循环中递进 / 重置。
#[derive(Debug, Clone)]
pub struct Backoff {
    policy: BackoffPolicy,
    attempt: u32,
}

impl Backoff {
    /// 用给定策略创建。
    pub fn new(policy: BackoffPolicy) -> Self {
        Self { policy, attempt: 0 }
    }

    /// 当前尝试次数（从 0 开始）。
    pub fn attempt(&self) -> u32 {
        self.attempt
    }

    /// 取得本次延迟（含抖动）并把尝试计数 +1。
    pub fn advance(&mut self, rand_unit: f64) -> Duration {
        let d = self.policy.next_delay(self.attempt, rand_unit);
        self.attempt = self.attempt.saturating_add(1);
        d
    }

    /// 连接成功后重置退避。
    pub fn reset(&mut self) {
        self.attempt = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> BackoffPolicy {
        BackoffPolicy {
            base: Duration::from_secs(1),
            max: Duration::from_secs(60),
            factor: 2,
            jitter_pct: 20,
        }
    }

    #[test]
    fn exponential_growth() {
        let p = policy();
        assert_eq!(p.delay_for_attempt(0), Duration::from_secs(1));
        assert_eq!(p.delay_for_attempt(1), Duration::from_secs(2));
        assert_eq!(p.delay_for_attempt(2), Duration::from_secs(4));
        assert_eq!(p.delay_for_attempt(3), Duration::from_secs(8));
        assert_eq!(p.delay_for_attempt(4), Duration::from_secs(16));
        assert_eq!(p.delay_for_attempt(5), Duration::from_secs(32));
    }

    #[test]
    fn caps_at_max() {
        let p = policy();
        // 64s > 60s 上限
        assert_eq!(p.delay_for_attempt(6), Duration::from_secs(60));
        assert_eq!(p.delay_for_attempt(100), Duration::from_secs(60));
    }

    #[test]
    fn no_overflow_on_huge_attempt() {
        let p = policy();
        // 不应 panic / 溢出
        assert_eq!(p.delay_for_attempt(u32::MAX), Duration::from_secs(60));
    }

    #[test]
    fn jitter_zero_returns_base() {
        let mut p = policy();
        p.jitter_pct = 0;
        assert_eq!(p.next_delay(2, 0.0), Duration::from_secs(4));
        assert_eq!(p.next_delay(2, 0.999), Duration::from_secs(4));
    }

    #[test]
    fn jitter_bounds() {
        let p = policy(); // jitter 20%
        let base = p.delay_for_attempt(3).as_millis() as f64; // 8000ms

        // rand=0.0 -> 偏移 -20% -> 6400ms
        let low = p.next_delay(3, 0.0);
        assert_eq!(low, Duration::from_millis((base * 0.8) as u64));

        // rand=0.5 -> 偏移 0 -> 8000ms
        let mid = p.next_delay(3, 0.5);
        assert_eq!(mid, Duration::from_millis(base as u64));

        // rand 趋近 1.0 -> 偏移 趋近 +20% -> ~9600ms
        let high = p.next_delay(3, 0.9999);
        assert!(high.as_millis() as f64 <= base * 1.2 + 1.0);
        assert!(high.as_millis() as f64 >= base * 1.19);
    }

    #[test]
    fn jitter_never_exceeds_max() {
        let p = policy();
        // 即便在上限处加正向抖动，也不得超过 max。
        let d = p.next_delay(100, 0.9999);
        assert!(d <= p.max);
    }

    #[test]
    fn jitter_never_negative() {
        let mut p = policy();
        p.jitter_pct = 100; // 极端抖动
        let d = p.next_delay(0, 0.0); // 偏移 -100% -> 0
        assert_eq!(d, Duration::from_millis(0));
    }

    #[test]
    fn rand_unit_clamped() {
        let p = policy();
        // 越界 rand 被裁剪，不 panic。
        let _ = p.next_delay(2, -5.0);
        let _ = p.next_delay(2, 5.0);
    }

    #[test]
    fn backoff_state_advances_and_resets() {
        let mut b = Backoff::new(policy());
        assert_eq!(b.attempt(), 0);
        b.advance(0.5);
        assert_eq!(b.attempt(), 1);
        b.advance(0.5);
        assert_eq!(b.attempt(), 2);
        b.reset();
        assert_eq!(b.attempt(), 0);
    }

    #[test]
    fn backoff_advance_uses_current_attempt() {
        let mut b = Backoff::new(policy());
        // 第 0 次 advance 用 attempt=0 -> base=1s (jitter mid)
        let d0 = b.advance(0.5);
        assert_eq!(d0, Duration::from_secs(1));
        // 第 1 次 advance 用 attempt=1 -> 2s
        let d1 = b.advance(0.5);
        assert_eq!(d1, Duration::from_secs(2));
    }
}
