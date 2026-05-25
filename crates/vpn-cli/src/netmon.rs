//! Story 4.17: 网络变化检测。
//!
//! 监听本机网络环境变化（出口 IP / 默认网关变化），变化时通知 daemon 触发
//! 快速重连（无需等待心跳超时）。
//!
//! 设计：
//! - [`NetworkProbe`] trait 抽象「一次网络快照采集」，便于注入 mock 与降级实现。
//! - 平台原生事件（macOS SCNetworkReachability / Linux netlink / Windows
//!   notification）成本高且需真机，本轮采用**轮询降级**（[`PollingMonitor`]）：
//!   定期采集 [`NetSnapshot`]，与上次比较，变化即触发回调。
//! - **可测纯逻辑**：[`NetSnapshot::changed_from`] 比较逻辑不依赖系统调用。

use std::time::Duration;

use async_trait::async_trait;

use crate::error::CliResult;

/// 一次网络环境快照（用于变化比较）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NetSnapshot {
    /// 本机主出口接口 IP（如 `192.168.1.23`）；不可得时为 None。
    pub local_ip: Option<String>,
    /// 默认网关（如 `192.168.1.1`）；不可得时为 None。
    pub default_gateway: Option<String>,
    /// 主接口名（如 `en0`）；用于检测接口切换（Wi-Fi <-> 有线）。
    pub primary_interface: Option<String>,
}

impl NetSnapshot {
    /// 纯逻辑：与上一快照比较，判断是否发生「需要重连」的变化。
    ///
    /// 规则：本地 IP、默认网关、主接口任一发生变化即视为变化。
    /// 从「全 None」到仍「全 None」不算变化（网络持续不可用，交给重连退避处理）。
    pub fn changed_from(&self, prev: &NetSnapshot) -> bool {
        self.local_ip != prev.local_ip
            || self.default_gateway != prev.default_gateway
            || self.primary_interface != prev.primary_interface
    }

    /// 是否处于「无网络」状态（出口 IP 与网关都不可得）。
    pub fn is_offline(&self) -> bool {
        self.local_ip.is_none() && self.default_gateway.is_none()
    }
}

/// 网络快照探针抽象。
#[async_trait]
pub trait NetworkProbe: Send + Sync {
    /// 采集当前网络快照。
    async fn snapshot(&self) -> CliResult<NetSnapshot>;
}

/// 基于轮询的网络变化监视器。
///
/// 周期性调用 `probe.snapshot()`，与上次比较，变化则调用 `on_change`。
pub struct PollingMonitor<P: NetworkProbe> {
    probe: P,
    interval: Duration,
    last: Option<NetSnapshot>,
}

impl<P: NetworkProbe> PollingMonitor<P> {
    /// 创建监视器。`interval` 为轮询周期（典型 2~5s）。
    pub fn new(probe: P, interval: Duration) -> Self {
        Self {
            probe,
            interval,
            last: None,
        }
    }

    /// 轮询周期。
    pub fn interval(&self) -> Duration {
        self.interval
    }

    /// 执行一次轮询：采集快照并与上次比较。返回 `Ok(true)` 表示检测到变化。
    ///
    /// 第一次调用仅记录基线，返回 `Ok(false)`。
    pub async fn poll_once(&mut self) -> CliResult<bool> {
        let current = self.probe.snapshot().await?;
        let changed = match &self.last {
            Some(prev) => current.changed_from(prev),
            None => false,
        };
        self.last = Some(current);
        Ok(changed)
    }

    /// 持续轮询循环，检测到变化时调用 `on_change`。
    ///
    /// 真机验证：长期运行于 daemon 后台任务中。
    pub async fn run<F>(&mut self, mut on_change: F) -> CliResult<()>
    where
        F: FnMut(),
    {
        let mut ticker = tokio::time::interval(self.interval);
        loop {
            ticker.tick().await;
            if self.poll_once().await? {
                on_change();
            }
        }
    }
}

/// 默认（轮询降级）出口探针：通过创建一个面向公网的 UDP socket 推断本机出口 IP。
///
/// 不发送任何数据包（`connect` 仅在内核选路，确定本地地址）。默认网关 / 接口名
/// 的精确获取需平台原生 API，本轮留作 None（出口 IP 变化已足以触发重连）。
pub struct OutboundIpProbe {
    /// 用于选路的目标地址（不会真正通信）。
    target: String,
}

impl Default for OutboundIpProbe {
    fn default() -> Self {
        // 使用一个公网 IP 作为「选路目标」（不发包，仅触发本地地址选择）。
        Self {
            target: "8.8.8.8:80".to_string(),
        }
    }
}

#[async_trait]
impl NetworkProbe for OutboundIpProbe {
    async fn snapshot(&self) -> CliResult<NetSnapshot> {
        let local_ip = detect_outbound_ip(&self.target);
        Ok(NetSnapshot {
            local_ip,
            default_gateway: None,
            primary_interface: None,
        })
    }
}

/// 通过 UDP `connect` 推断本机出口 IP（不发包）。失败返回 None。
fn detect_outbound_ip(target: &str) -> Option<String> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect(target).ok()?;
    sock.local_addr().ok().map(|a| a.ip().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn snap(ip: Option<&str>, gw: Option<&str>, iface: Option<&str>) -> NetSnapshot {
        NetSnapshot {
            local_ip: ip.map(String::from),
            default_gateway: gw.map(String::from),
            primary_interface: iface.map(String::from),
        }
    }

    #[test]
    fn no_change_when_identical() {
        let a = snap(Some("192.168.1.2"), Some("192.168.1.1"), Some("en0"));
        let b = a.clone();
        assert!(!b.changed_from(&a));
    }

    #[test]
    fn ip_change_detected() {
        let a = snap(Some("192.168.1.2"), Some("192.168.1.1"), Some("en0"));
        let b = snap(Some("10.0.0.5"), Some("192.168.1.1"), Some("en0"));
        assert!(b.changed_from(&a));
    }

    #[test]
    fn gateway_change_detected() {
        let a = snap(Some("192.168.1.2"), Some("192.168.1.1"), Some("en0"));
        let b = snap(Some("192.168.1.2"), Some("10.0.0.1"), Some("en0"));
        assert!(b.changed_from(&a));
    }

    #[test]
    fn interface_switch_detected() {
        let a = snap(Some("192.168.1.2"), Some("192.168.1.1"), Some("en0"));
        let b = snap(Some("192.168.1.2"), Some("192.168.1.1"), Some("en1"));
        assert!(b.changed_from(&a));
    }

    #[test]
    fn going_offline_is_a_change() {
        let a = snap(Some("192.168.1.2"), Some("192.168.1.1"), Some("en0"));
        let b = snap(None, None, None);
        assert!(b.changed_from(&a));
        assert!(b.is_offline());
    }

    #[test]
    fn staying_offline_is_not_a_change() {
        let a = snap(None, None, None);
        let b = snap(None, None, None);
        assert!(!b.changed_from(&a));
    }

    // === Mock probe 驱动 PollingMonitor 的纯逻辑测试 ===

    struct ScriptedProbe {
        snaps: Vec<NetSnapshot>,
        idx: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl NetworkProbe for ScriptedProbe {
        async fn snapshot(&self) -> CliResult<NetSnapshot> {
            let i = self.idx.fetch_add(1, Ordering::SeqCst);
            Ok(self.snaps[i.min(self.snaps.len() - 1)].clone())
        }
    }

    #[tokio::test]
    async fn first_poll_sets_baseline_no_change() {
        let probe = ScriptedProbe {
            snaps: vec![snap(Some("1.1.1.1"), None, None)],
            idx: Arc::new(AtomicUsize::new(0)),
        };
        let mut mon = PollingMonitor::new(probe, Duration::from_secs(1));
        assert!(!mon.poll_once().await.unwrap(), "首次轮询应仅设基线");
    }

    #[tokio::test]
    async fn detects_change_across_polls() {
        let probe = ScriptedProbe {
            snaps: vec![
                snap(Some("1.1.1.1"), None, None),
                snap(Some("1.1.1.1"), None, None), // 不变
                snap(Some("2.2.2.2"), None, None), // 变化
            ],
            idx: Arc::new(AtomicUsize::new(0)),
        };
        let mut mon = PollingMonitor::new(probe, Duration::from_millis(1));
        assert!(!mon.poll_once().await.unwrap()); // 基线
        assert!(!mon.poll_once().await.unwrap()); // 不变
        assert!(mon.poll_once().await.unwrap()); // 变化
    }

    #[tokio::test]
    async fn run_invokes_callback_on_change() {
        let probe = ScriptedProbe {
            snaps: vec![
                snap(Some("1.1.1.1"), None, None),
                snap(Some("2.2.2.2"), None, None),
            ],
            idx: Arc::new(AtomicUsize::new(0)),
        };
        let mut mon = PollingMonitor::new(probe, Duration::from_millis(1));
        // 手动驱动两次 poll，验证回调触发逻辑（不进死循环）。
        let mut fired = 0;
        if mon.poll_once().await.unwrap() {
            fired += 1;
        }
        if mon.poll_once().await.unwrap() {
            fired += 1;
        }
        assert_eq!(fired, 1);
    }

    #[test]
    fn monitor_exposes_interval() {
        let mon = PollingMonitor::new(OutboundIpProbe::default(), Duration::from_secs(3));
        assert_eq!(mon.interval(), Duration::from_secs(3));
    }
}
