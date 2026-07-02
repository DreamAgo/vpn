//! WireGuard 控制平面抽象。
//!
//! [`WireGuardControl`] 把「在内核/用户态 WireGuard 上增删 peer」隔离成 trait，
//! 让 vpn-server 的 peer 业务逻辑无需关心后端实现，也便于在无 root 环境
//! （CI / 单元测试 / 开发机）用 [`NoopWireGuardControl`] 注入。
//!
//! 真实后端（基于 boringtun 用户态隧道 + UDP socket + TUN）属于「系统集成层」，
//! 需要 CAP_NET_ADMIN 与真实网卡，留待目标平台上实现并真机验证。
//! 见 [`BoringTunControl`]（当前为占位骨架）。

use std::collections::BTreeSet;
use std::sync::Mutex;

use async_trait::async_trait;
use vpn_core::Result;

use crate::config::WgPeerConfig;

/// WireGuard 控制平面：增删查 peer。
#[async_trait]
pub trait WireGuardControl: Send + Sync {
    /// 配置（新增或更新）一个 peer。
    async fn configure_peer(&self, cfg: &WgPeerConfig) -> Result<()>;
    /// 移除一个 peer（按公钥）。
    async fn remove_peer(&self, public_key: &str) -> Result<()>;
    /// 列出当前已配置的 peer 公钥。
    async fn list_peers(&self) -> Result<Vec<String>>;
    /// 删除若干站点 LAN 网段对应的 OS 路由（`<subnet> dev <iface>`）。
    ///
    /// 某 peer 的 routed_subnets 被**缩减**时，`configure_peer` 的 `ip route replace`
    /// 只会重铺当前网段、不会删除已移除网段的旧路由，残留路由会把流量黑洞到接口上
    /// 已无对应 allowed-ips 的 peer。故缩减路由时需显式删除。默认无操作（Noop/测试后端）。
    async fn remove_routes(&self, _subnets: &[String]) -> Result<()> {
        Ok(())
    }
    /// 服务端公钥（base64）。
    fn server_public_key(&self) -> &str;
}

/// 无副作用实现：在内存里记账，用于测试与无 root 运行。
///
/// 行为与真实后端语义一致（幂等增删、可列举），但不触碰任何网络接口。
pub struct NoopWireGuardControl {
    server_public_key: String,
    peers: Mutex<BTreeSet<String>>,
}

impl NoopWireGuardControl {
    pub fn new(server_public_key: impl Into<String>) -> Self {
        Self {
            server_public_key: server_public_key.into(),
            peers: Mutex::new(BTreeSet::new()),
        }
    }
}

#[async_trait]
impl WireGuardControl for NoopWireGuardControl {
    async fn configure_peer(&self, cfg: &WgPeerConfig) -> Result<()> {
        self.peers.lock().unwrap().insert(cfg.public_key.clone());
        tracing::debug!(public_key = %cfg.public_key, vpn_ip = %cfg.vpn_ip, "noop configure_peer");
        Ok(())
    }

    async fn remove_peer(&self, public_key: &str) -> Result<()> {
        self.peers.lock().unwrap().remove(public_key);
        tracing::debug!(public_key, "noop remove_peer");
        Ok(())
    }

    async fn list_peers(&self) -> Result<Vec<String>> {
        Ok(self.peers.lock().unwrap().iter().cloned().collect())
    }

    fn server_public_key(&self) -> &str {
        &self.server_public_key
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WgPeerConfig;

    fn cfg(pk: &str, ip: &str) -> WgPeerConfig {
        WgPeerConfig {
            public_key: pk.to_string(),
            vpn_ip: ip.parse().unwrap(),
            endpoint: None,
            allowed_subnets: Vec::new(),
        }
    }

    #[tokio::test]
    async fn noop_tracks_peers() {
        let wg = NoopWireGuardControl::new("SERVER_PUB");
        assert_eq!(wg.server_public_key(), "SERVER_PUB");
        wg.configure_peer(&cfg("PK1", "10.8.0.2")).await.unwrap();
        wg.configure_peer(&cfg("PK2", "10.8.0.3")).await.unwrap();
        let peers = wg.list_peers().await.unwrap();
        assert_eq!(peers, vec!["PK1".to_string(), "PK2".to_string()]);

        wg.remove_peer("PK1").await.unwrap();
        assert_eq!(wg.list_peers().await.unwrap(), vec!["PK2".to_string()]);
    }

    #[tokio::test]
    async fn configure_peer_is_idempotent() {
        let wg = NoopWireGuardControl::new("S");
        wg.configure_peer(&cfg("PK1", "10.8.0.2")).await.unwrap();
        wg.configure_peer(&cfg("PK1", "10.8.0.2")).await.unwrap();
        assert_eq!(wg.list_peers().await.unwrap().len(), 1);
    }
}
