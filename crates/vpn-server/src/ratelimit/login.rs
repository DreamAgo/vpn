//! 登录失败计数与账号锁定（内存实现）。
//!
//! 设计：
//! - key = username（也支持 IP，但优先 username 防止单 IP 多账号攻击 + 跨 IP 攻击单账号）
//! - 5 次失败 → 锁定 15 分钟（指数退避：第 6 次失败后锁 15min、30min、60min、120min、240min）
//! - 锁定期间任何登录请求直接返回 AccountLocked，不验证密码

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use tokio::sync::RwLock;

const MAX_BEFORE_LOCK: u32 = 5;

#[derive(Debug, Clone)]
pub struct AttemptRecord {
    pub failed_count: u32,
    pub locked_until: Option<DateTime<Utc>>,
}

impl AttemptRecord {
    fn new() -> Self {
        Self {
            failed_count: 0,
            locked_until: None,
        }
    }

    fn is_locked(&self, now: DateTime<Utc>) -> bool {
        self.locked_until.is_some_and(|until| now < until)
    }
}

/// 失败计数器（在 AppState 中共享一个实例）。
#[derive(Debug, Clone, Default)]
pub struct LoginAttempts {
    inner: Arc<RwLock<HashMap<String, AttemptRecord>>>,
}

impl LoginAttempts {
    pub fn new() -> Self {
        Self::default()
    }

    /// 检查是否被锁定。
    pub async fn is_locked(&self, key: &str, now: DateTime<Utc>) -> bool {
        self.inner
            .read()
            .await
            .get(key)
            .map(|r| r.is_locked(now))
            .unwrap_or(false)
    }

    /// 登录成功后清空该 key 的失败记录。
    pub async fn reset(&self, key: &str) {
        self.inner.write().await.remove(key);
    }

    /// 登录失败后递增计数；达到阈值后锁定（指数退避）。
    /// 返回当前是否处于锁定状态。
    pub async fn record_failure(&self, key: &str, now: DateTime<Utc>) -> bool {
        let mut map = self.inner.write().await;
        let entry = map
            .entry(key.to_string())
            .or_insert_with(AttemptRecord::new);
        entry.failed_count += 1;
        if entry.failed_count >= MAX_BEFORE_LOCK {
            // 指数退避：5次=15min, 6次=30min, 7次=60min, 8次=120min, 9次+=240min
            let lock_minutes = match entry.failed_count {
                MAX_BEFORE_LOCK => 15,
                v if v == MAX_BEFORE_LOCK + 1 => 30,
                v if v == MAX_BEFORE_LOCK + 2 => 60,
                v if v == MAX_BEFORE_LOCK + 3 => 120,
                _ => 240,
            };
            entry.locked_until = Some(now + chrono::Duration::minutes(lock_minutes));
            tracing::warn!(key = %key, count = entry.failed_count, lock_minutes, "账号锁定");
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn no_failures_means_not_locked() {
        let attempts = LoginAttempts::new();
        assert!(!attempts.is_locked("alice", Utc::now()).await);
    }

    #[tokio::test]
    async fn four_failures_no_lock() {
        let attempts = LoginAttempts::new();
        let now = Utc::now();
        for _ in 0..4 {
            attempts.record_failure("alice", now).await;
        }
        assert!(!attempts.is_locked("alice", now).await);
    }

    #[tokio::test]
    async fn five_failures_triggers_lock() {
        let attempts = LoginAttempts::new();
        let now = Utc::now();
        for _ in 0..5 {
            attempts.record_failure("alice", now).await;
        }
        assert!(attempts.is_locked("alice", now).await);
    }

    #[tokio::test]
    async fn reset_clears_lock() {
        let attempts = LoginAttempts::new();
        let now = Utc::now();
        for _ in 0..5 {
            attempts.record_failure("alice", now).await;
        }
        attempts.reset("alice").await;
        assert!(!attempts.is_locked("alice", now).await);
    }

    #[tokio::test]
    async fn lock_expires_after_window() {
        let attempts = LoginAttempts::new();
        let t0 = Utc::now();
        for _ in 0..5 {
            attempts.record_failure("alice", t0).await;
        }
        // 16 分钟后已过 15 分钟锁定窗口
        let t1 = t0 + chrono::Duration::minutes(16);
        assert!(!attempts.is_locked("alice", t1).await);
    }
}
