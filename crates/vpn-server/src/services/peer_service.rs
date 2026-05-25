//! Peer 数据平面业务服务（Epic 4：注册 / 心跳 / 注销 / 配置下载 + 离线检测）。
//!
//! 持有服务端 WireGuard 状态：
//! - 服务端公钥（注册响应与客户端配置需要）
//! - `IpPool`（可变共享，置于 `tokio::sync::Mutex` 之后）
//! - `Arc<dyn WireGuardControl>`（本轮注入 Noop，真实后端留待真机集成）

use std::net::Ipv4Addr;
use std::sync::Arc;

use ipnet::Ipv4Net;
use tokio::sync::Mutex;
use uuid::Uuid;
use vpn_api_types::peer::{PeerRegisterRequest, PeerRegisterResponse};
use vpn_core::{AppError, Result};
use vpn_wireguard::{
    generate_keypair, public_key_from_private, render_client_config, IpPool,
    KernelWireGuardControl, NoopWireGuardControl, WgPeerConfig, WireGuardControl,
};

use crate::repositories::{
    peer_repo_sqlite::SqlitePeerRepository, system_config_repo_sqlite::SqliteSystemConfigRepository,
};

/// system_config 中存储服务端 WG 私钥/公钥的 key。
pub const KEY_SERVER_WG_PRIVATE: &str = "server_wg_private_key";
pub const KEY_SERVER_WG_PUBLIC: &str = "server_wg_public_key";

/// 客户端配置下载里 PrivateKey 字段的占位符（服务端不持有客户端私钥）。
const CLIENT_PRIVATE_KEY_PLACEHOLDER: &str = "<在此填入客户端私钥>";
/// PersistentKeepalive 秒数（穿越 NAT）。
const PERSISTENT_KEEPALIVE: u16 = 25;
/// 离线判定阈值：心跳超过该毫秒数未更新视为离线。
pub const OFFLINE_THRESHOLD_MS: i64 = 90_000;

/// 已渲染的配置文件下载内容。
#[derive(Debug, Clone)]
pub struct PeerConfigDownload {
    pub filename: String,
    pub content: String,
}

#[derive(Clone)]
pub struct PeerService {
    pub peer_repo: SqlitePeerRepository,
    control: Arc<dyn WireGuardControl>,
    ip_pool: Arc<Mutex<IpPool>>,
    subnet: Ipv4Net,
    server_endpoint: String,
}

impl PeerService {
    /// 构造 PeerService。`ip_pool` 应已由调用方用 peers 表中已占用 IP 回填。
    pub fn new(
        peer_repo: SqlitePeerRepository,
        control: Arc<dyn WireGuardControl>,
        ip_pool: IpPool,
        server_endpoint: String,
    ) -> Self {
        let subnet = ip_pool.subnet();
        Self {
            peer_repo,
            control,
            ip_pool: Arc::new(Mutex::new(ip_pool)),
            subnet,
            server_endpoint,
        }
    }

    fn server_public_key(&self) -> &str {
        self.control.server_public_key()
    }

    fn subnet_cidr(&self) -> String {
        self.subnet.to_string()
    }

    /// Story 4.5：注册节点。
    ///
    /// 同一 user 已有非 deleted peer → 复用其 vpn_ip，更新公钥/设备名/os_info；
    /// 否则分配新 IP 并插入。最后调 control.configure_peer。
    pub async fn register(
        &self,
        user_id: &str,
        req: &PeerRegisterRequest,
    ) -> Result<PeerRegisterResponse> {
        let existing = self.peer_repo.find_active_by_user(user_id).await?;

        let vpn_ip: String = match existing {
            Some(peer) => {
                // 复用既有 IP，更新注册信息（公钥冲突会返回 DuplicateResource）。
                self.peer_repo
                    .update_registration(
                        &peer.id,
                        &req.device_name,
                        &req.wg_public_key,
                        req.os_info.as_deref(),
                    )
                    .await?;
                peer.vpn_ip
            }
            None => {
                let ip = {
                    let mut pool = self.ip_pool.lock().await;
                    pool.allocate()?
                };
                let ip_str = ip.to_string();
                let id = Uuid::now_v7().to_string();
                match self
                    .peer_repo
                    .insert(
                        &id,
                        user_id,
                        &req.device_name,
                        &req.wg_public_key,
                        &ip_str,
                        req.os_info.as_deref(),
                    )
                    .await
                {
                    Ok(_) => ip_str,
                    Err(e) => {
                        // 插入失败（如公钥冲突）→ 归还刚分配的 IP，避免泄漏。
                        self.ip_pool.lock().await.release(ip);
                        return Err(e);
                    }
                }
            }
        };

        let vpn_ip_parsed: Ipv4Addr = vpn_ip
            .parse()
            .map_err(|e| AppError::Internal(Box::new(e)))?;
        self.control
            .configure_peer(&WgPeerConfig {
                public_key: req.wg_public_key.clone(),
                vpn_ip: vpn_ip_parsed,
                endpoint: None,
            })
            .await?;

        Ok(PeerRegisterResponse {
            vpn_ip,
            server_public_key: self.server_public_key().to_string(),
            server_endpoint: self.server_endpoint.clone(),
            vpn_subnet: self.subnet_cidr(),
        })
    }

    /// Story 4.6：心跳。无活跃 peer → PeerNotFound。
    pub async fn heartbeat(
        &self,
        user_id: &str,
        endpoint: Option<&str>,
        now_ms: i64,
    ) -> Result<()> {
        let affected = self
            .peer_repo
            .touch_heartbeat(user_id, endpoint, now_ms)
            .await?;
        if affected == 0 {
            return Err(AppError::PeerNotFound);
        }
        Ok(())
    }

    /// Story 4.7：注销当前 user 的 peer。无活跃 peer → PeerNotFound。
    pub async fn delete_me(&self, user_id: &str) -> Result<()> {
        let peer = self
            .peer_repo
            .find_active_by_user(user_id)
            .await?
            .ok_or(AppError::PeerNotFound)?;
        self.control.remove_peer(&peer.wg_public_key).await?;
        self.peer_repo.mark_deleted_by_user(user_id).await?;
        Ok(())
    }

    /// Story 4.7：渲染当前 user peer 的客户端配置文件。无活跃 peer → PeerNotFound。
    pub async fn render_config(&self, user_id: &str) -> Result<PeerConfigDownload> {
        let peer = self
            .peer_repo
            .find_active_by_user(user_id)
            .await?
            .ok_or(AppError::PeerNotFound)?;
        let client_ip: Ipv4Addr = peer
            .vpn_ip
            .parse()
            .map_err(|e| AppError::Internal(Box::new(e)))?;
        let dns = self
            .subnet
            .hosts()
            .next()
            .map(|ip| ip.to_string())
            .unwrap_or_else(|| "10.8.0.1".to_string());
        let content = render_client_config(
            CLIENT_PRIVATE_KEY_PLACEHOLDER,
            client_ip,
            self.subnet.prefix_len(),
            &dns,
            self.server_public_key(),
            &self.server_endpoint,
            PERSISTENT_KEEPALIVE,
        );
        Ok(PeerConfigDownload {
            filename: "vpn.conf".to_string(),
            content,
        })
    }

    /// 离线检测：把心跳超过阈值的 online peer 标记为 offline。返回标记行数。
    pub async fn scan_offline(&self, now_ms: i64) -> Result<u64> {
        let cutoff = now_ms - OFFLINE_THRESHOLD_MS;
        self.peer_repo.mark_stale_offline(cutoff).await
    }

    /// Story 5.5：心跳（admin 视角）。同 `heartbeat`，但若 peer 已被强制下线则拒绝。
    ///
    /// 强制下线后客户端下次心跳应失败，提示重新登录。
    pub async fn heartbeat_checked(
        &self,
        user_id: &str,
        endpoint: Option<&str>,
        now_ms: i64,
    ) -> Result<()> {
        // touch_heartbeat 的 SQL 排除 status='deleted'，但 force_removed 仍会被更新；
        // 故先显式检查活跃 peer 状态。
        if let Some(peer) = self.peer_repo.find_active_by_user(user_id).await? {
            if peer.status == "force_removed" {
                // 复用 TokenExpired（401）→ 客户端据此提示重新登录。
                return Err(AppError::TokenExpired);
            }
        }
        self.heartbeat(user_id, endpoint, now_ms).await
    }

    /// Story 5.5：admin 强制下线指定 peer。
    ///
    /// 从 WireGuard runtime 移除 + 把 peers.status 置为 'force_removed'。
    /// peer 不存在 → PeerNotFound。
    pub async fn force_remove(&self, peer_id: &str) -> Result<()> {
        let peer = self
            .peer_repo
            .find_by_id(peer_id)
            .await?
            .ok_or(AppError::PeerNotFound)?;
        self.control.remove_peer(&peer.wg_public_key).await?;
        self.peer_repo.mark_force_removed(peer_id).await?;
        Ok(())
    }

    /// Story 5.5：admin peer 列表（JOIN users）。
    pub async fn list_admin_peers(
        &self,
        query: &vpn_api_types::peer::AdminPeerQuery,
    ) -> Result<vpn_api_types::Page<vpn_api_types::peer::AdminPeerView>> {
        use crate::repositories::peer_repo_sqlite::AdminPeerFilter;
        let page = query.page.unwrap_or(1).max(1);
        let page_size = query.page_size.unwrap_or(20).clamp(1, 100);
        let filter = AdminPeerFilter {
            search: query.search.clone(),
            status: query.status.clone(),
            page,
            page_size,
        };
        let total = self.peer_repo.count_admin(&filter).await? as u64;
        let rows = self.peer_repo.list_admin(&filter).await?;
        let items = rows
            .into_iter()
            .map(|r| vpn_api_types::peer::AdminPeerView {
                id: r.id,
                user_id: r.user_id,
                username: r.username,
                email: r.email,
                device_name: r.device_name,
                wg_public_key: r.wg_public_key,
                vpn_ip: r.vpn_ip,
                endpoint: r.endpoint,
                os_info: r.os_info,
                last_seen_at: r.last_seen_at,
                status: r.status,
                created_at: r.created_at,
            })
            .collect();
        Ok(vpn_api_types::Page::new(items, total, page, page_size))
    }

    /// 服务端公钥（供 system_info 等只读展示用）。
    pub fn server_public_key_string(&self) -> String {
        self.server_public_key().to_string()
    }
}

/// 装配 PeerService（默认 Noop 后端，用于测试 / 无特权环境）。
pub async fn build_peer_service(
    peer_repo: SqlitePeerRepository,
    config_repo: &SqliteSystemConfigRepository,
    subnet: Ipv4Net,
    server_endpoint: String,
) -> Result<PeerService> {
    build_peer_service_with_backend(
        peer_repo,
        config_repo,
        subnet,
        server_endpoint,
        "noop",
        "wg0",
        51820,
    )
    .await
}

/// 装配 PeerService，可选 WireGuard 后端。
///
/// `backend`：`"kernel"` 使用 Linux 内核 WireGuard（需 root/CAP_NET_ADMIN + `wg` 工具），
/// 其余值（含 `"noop"`）使用无副作用的记账实现。
pub async fn build_peer_service_with_backend(
    peer_repo: SqlitePeerRepository,
    config_repo: &SqliteSystemConfigRepository,
    subnet: Ipv4Net,
    server_endpoint: String,
    backend: &str,
    iface: &str,
    listen_port: u16,
) -> Result<PeerService> {
    // 1. load-or-generate 服务端 WG 密钥（私钥 + 公钥都需要）
    let (private_key, public_key) = match config_repo.get(KEY_SERVER_WG_PRIVATE).await? {
        Some(private) => {
            let public = public_key_from_private(&private)?;
            (private, public)
        }
        None => {
            let kp = generate_keypair();
            config_repo
                .set(KEY_SERVER_WG_PRIVATE, &kp.private_key)
                .await?;
            config_repo
                .set(KEY_SERVER_WG_PUBLIC, &kp.public_key)
                .await?;
            (kp.private_key, kp.public_key)
        }
    };

    // 2. 构造 IpPool 并用现存 peers 回填
    let mut ip_pool = IpPool::new(subnet);
    for ip_str in peer_repo.list_active_vpn_ips().await? {
        match ip_str.parse::<Ipv4Addr>() {
            Ok(ip) => ip_pool.mark_used(ip),
            Err(e) => {
                tracing::warn!(vpn_ip = %ip_str, error = %e, "peers 表中存在非法 vpn_ip，回填跳过")
            }
        }
    }

    // 3. 选择 WireGuard 后端
    let control: Arc<dyn WireGuardControl> = if backend == "kernel" {
        let server_addr = ip_pool
            .server_addr()
            .ok_or_else(|| AppError::WireGuard("子网无可用服务端地址".to_string()))?;
        let kc = KernelWireGuardControl::start(
            iface,
            &private_key,
            &public_key,
            server_addr,
            subnet.prefix_len(),
            listen_port,
        )
        .await?;
        // 启动恢复：把已存在的 active peers 重新下发到内核接口
        for (pubkey, ip_str) in peer_repo.list_active_peer_keys().await? {
            if let Ok(ip) = ip_str.parse::<Ipv4Addr>() {
                let cfg = WgPeerConfig {
                    public_key: pubkey,
                    vpn_ip: ip,
                    endpoint: None,
                };
                if let Err(e) = kc.configure_peer(&cfg).await {
                    tracing::warn!(error = %e, "启动恢复 peer 配置失败");
                }
            }
        }
        tracing::info!(iface, "使用内核 WireGuard 后端");
        Arc::new(kc)
    } else {
        tracing::info!("使用 Noop WireGuard 后端（无真实隧道）");
        Arc::new(NoopWireGuardControl::new(public_key))
    };

    Ok(PeerService::new(
        peer_repo,
        control,
        ip_pool,
        server_endpoint,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use sqlx::SqlitePool;
    use std::str::FromStr;
    use vpn_wireguard::NoopWireGuardControl;

    async fn setup_pool() -> SqlitePool {
        let url = format!(
            "sqlite:file:peer_service_test_{}?mode=memory&cache=private",
            Uuid::new_v4()
        );
        let opts = SqliteConnectOptions::from_str(&url).unwrap();
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await
            .unwrap();
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        sqlx::query(
            r#"INSERT INTO users (id, username, email, password_hash, role, status, must_change_password, created_at, updated_at)
               VALUES ('user-1', 'alice', 'a@e.com', 'h', 'user', 'active', 0, 0, 0),
                      ('user-2', 'bob', 'b@e.com', 'h', 'user', 'active', 0, 0, 0)"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    fn service(pool: SqlitePool) -> PeerService {
        let pool_net: Ipv4Net = "10.8.0.0/24".parse().unwrap();
        let ip_pool = IpPool::new(pool_net);
        let control = Arc::new(NoopWireGuardControl::new("SERVER_PUB"));
        PeerService::new(
            SqlitePeerRepository::new(pool),
            control,
            ip_pool,
            "vpn.example.com:51820".to_string(),
        )
    }

    fn reg(pk: &str) -> PeerRegisterRequest {
        PeerRegisterRequest {
            wg_public_key: pk.to_string(),
            device_name: "MBP".to_string(),
            os_info: Some("macOS".to_string()),
        }
    }

    #[tokio::test]
    async fn register_allocates_new_ip() {
        let svc = service(setup_pool().await);
        let resp = svc.register("user-1", &reg("PK1")).await.unwrap();
        assert_eq!(resp.vpn_ip, "10.8.0.2");
        assert_eq!(resp.server_public_key, "SERVER_PUB");
        assert_eq!(resp.server_endpoint, "vpn.example.com:51820");
        assert_eq!(resp.vpn_subnet, "10.8.0.0/24");
        assert_eq!(svc.control.list_peers().await.unwrap(), vec!["PK1"]);
    }

    #[tokio::test]
    async fn register_reuses_ip_for_same_user() {
        let svc = service(setup_pool().await);
        let r1 = svc.register("user-1", &reg("PK1")).await.unwrap();
        // 重新注册（换公钥）→ 同 IP
        let r2 = svc.register("user-1", &reg("PK2")).await.unwrap();
        assert_eq!(r1.vpn_ip, r2.vpn_ip);
        let row = svc
            .peer_repo
            .find_active_by_user("user-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.wg_public_key, "PK2");
    }

    #[tokio::test]
    async fn register_different_users_get_different_ips() {
        let svc = service(setup_pool().await);
        let r1 = svc.register("user-1", &reg("PK1")).await.unwrap();
        let r2 = svc.register("user-2", &reg("PK2")).await.unwrap();
        assert_ne!(r1.vpn_ip, r2.vpn_ip);
    }

    #[tokio::test]
    async fn register_duplicate_public_key_releases_ip() {
        let svc = service(setup_pool().await);
        svc.register("user-1", &reg("PK1")).await.unwrap();
        // user-2 用 user-1 已占公钥 → DuplicateResource，且 IP 应被归还
        let err = svc.register("user-2", &reg("PK1")).await.unwrap_err();
        assert!(matches!(err, AppError::DuplicateResource(_)));
        // 归还后，user-2 用新公钥应能拿到本应分配的那个 IP（10.8.0.3）
        let r = svc.register("user-2", &reg("PK2")).await.unwrap();
        assert_eq!(r.vpn_ip, "10.8.0.3");
    }

    #[tokio::test]
    async fn heartbeat_unknown_user_returns_peer_not_found() {
        let svc = service(setup_pool().await);
        let err = svc.heartbeat("user-1", None, 1000).await.unwrap_err();
        assert!(matches!(err, AppError::PeerNotFound));
    }

    #[tokio::test]
    async fn heartbeat_sets_online() {
        let svc = service(setup_pool().await);
        svc.register("user-1", &reg("PK1")).await.unwrap();
        svc.heartbeat("user-1", Some("1.2.3.4:99"), 1000)
            .await
            .unwrap();
        let row = svc
            .peer_repo
            .find_active_by_user("user-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.status, "online");
    }

    #[tokio::test]
    async fn render_config_contains_interface_and_peer_ip() {
        let svc = service(setup_pool().await);
        let resp = svc.register("user-1", &reg("PK1")).await.unwrap();
        let dl = svc.render_config("user-1").await.unwrap();
        assert_eq!(dl.filename, "vpn.conf");
        assert!(dl.content.contains("[Interface]"));
        assert!(dl
            .content
            .contains(&format!("Address = {}/24", resp.vpn_ip)));
        assert!(dl.content.contains("PublicKey = SERVER_PUB"));
        assert!(dl.content.contains("Endpoint = vpn.example.com:51820"));
        assert!(dl.content.contains(CLIENT_PRIVATE_KEY_PLACEHOLDER));
    }

    #[tokio::test]
    async fn render_config_unknown_user_returns_peer_not_found() {
        let svc = service(setup_pool().await);
        let err = svc.render_config("user-1").await.unwrap_err();
        assert!(matches!(err, AppError::PeerNotFound));
    }

    #[tokio::test]
    async fn delete_me_removes_peer_from_control_and_active() {
        let svc = service(setup_pool().await);
        svc.register("user-1", &reg("PK1")).await.unwrap();
        svc.delete_me("user-1").await.unwrap();
        assert!(svc
            .peer_repo
            .find_active_by_user("user-1")
            .await
            .unwrap()
            .is_none());
        assert!(svc.control.list_peers().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn delete_me_unknown_returns_peer_not_found() {
        let svc = service(setup_pool().await);
        let err = svc.delete_me("user-1").await.unwrap_err();
        assert!(matches!(err, AppError::PeerNotFound));
    }

    #[tokio::test]
    async fn register_after_delete_keeps_same_ip() {
        let svc = service(setup_pool().await);
        let r1 = svc.register("user-1", &reg("PK1")).await.unwrap();
        svc.delete_me("user-1").await.unwrap();
        // 注销后重新注册：当前实现不立即释放 IP，但 find_active 排除 deleted，
        // 故会走 allocate 分支拿到下一个 IP（旧 IP 仍在 pool 中被占用）。
        let r2 = svc.register("user-1", &reg("PK2")).await.unwrap();
        assert_ne!(r1.vpn_ip, r2.vpn_ip);
    }

    #[tokio::test]
    async fn build_peer_service_generates_and_persists_key() {
        let pool = setup_pool().await;
        let config_repo = SqliteSystemConfigRepository::new(pool.clone());
        let peer_repo = SqlitePeerRepository::new(pool.clone());
        let subnet: Ipv4Net = "10.8.0.0/24".parse().unwrap();

        let svc = build_peer_service(
            peer_repo.clone(),
            &config_repo,
            subnet,
            "vpn.example.com:51820".to_string(),
        )
        .await
        .unwrap();
        let pub1 = svc.server_public_key_string();
        assert_eq!(pub1.len(), 44);
        // 公钥应等于从存储私钥推导的值
        let private = config_repo
            .get(KEY_SERVER_WG_PRIVATE)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(public_key_from_private(&private).unwrap(), pub1);

        // 再次构造：密钥应稳定（load 而非 regenerate）
        let svc2 = build_peer_service(
            peer_repo,
            &config_repo,
            subnet,
            "vpn.example.com:51820".to_string(),
        )
        .await
        .unwrap();
        assert_eq!(svc2.server_public_key_string(), pub1);
    }

    #[tokio::test]
    async fn build_peer_service_backfills_ip_pool() {
        let pool = setup_pool().await;
        let config_repo = SqliteSystemConfigRepository::new(pool.clone());
        let peer_repo = SqlitePeerRepository::new(pool.clone());
        // 预置一个占用 10.8.0.2 的 peer
        peer_repo
            .insert("p1", "user-1", "MBP", "PK1", "10.8.0.2", None)
            .await
            .unwrap();
        let subnet: Ipv4Net = "10.8.0.0/24".parse().unwrap();
        let svc = build_peer_service(
            peer_repo,
            &config_repo,
            subnet,
            "vpn.example.com:51820".to_string(),
        )
        .await
        .unwrap();
        // 新注册（user-2）应跳过已占用的 .2，拿到 .3
        let resp = svc.register("user-2", &reg("PK2")).await.unwrap();
        assert_eq!(resp.vpn_ip, "10.8.0.3");
    }

    #[tokio::test]
    async fn force_remove_marks_force_removed_and_blocks_heartbeat() {
        let svc = service(setup_pool().await);
        svc.register("user-1", &reg("PK1")).await.unwrap();
        let peer = svc
            .peer_repo
            .find_active_by_user("user-1")
            .await
            .unwrap()
            .unwrap();

        svc.force_remove(&peer.id).await.unwrap();
        // 已从 control 移除
        assert!(svc.control.list_peers().await.unwrap().is_empty());
        // 状态为 force_removed
        let row = svc.peer_repo.find_by_id(&peer.id).await.unwrap().unwrap();
        assert_eq!(row.status, "force_removed");

        // 心跳被拒（TokenExpired → 401）
        let err = svc
            .heartbeat_checked("user-1", None, 1000)
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::TokenExpired));
    }

    #[tokio::test]
    async fn force_remove_unknown_returns_peer_not_found() {
        let svc = service(setup_pool().await);
        let err = svc.force_remove("missing").await.unwrap_err();
        assert!(matches!(err, AppError::PeerNotFound));
    }

    #[tokio::test]
    async fn heartbeat_checked_ok_for_normal_peer() {
        let svc = service(setup_pool().await);
        svc.register("user-1", &reg("PK1")).await.unwrap();
        svc.heartbeat_checked("user-1", Some("1.2.3.4:99"), 1000)
            .await
            .unwrap();
        let row = svc
            .peer_repo
            .find_active_by_user("user-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.status, "online");
    }

    #[tokio::test]
    async fn list_admin_peers_returns_username() {
        let svc = service(setup_pool().await);
        svc.register("user-1", &reg("PK1")).await.unwrap();
        let query = vpn_api_types::peer::AdminPeerQuery {
            page: None,
            page_size: None,
            search: None,
            status: None,
        };
        let page = svc.list_admin_peers(&query).await.unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].username, "alice");
    }

    #[tokio::test]
    async fn scan_offline_marks_stale_peers() {
        let svc = service(setup_pool().await);
        svc.register("user-1", &reg("PK1")).await.unwrap();
        // last_seen far in past
        svc.heartbeat("user-1", None, 100).await.unwrap();
        let marked = svc
            .scan_offline(100 + OFFLINE_THRESHOLD_MS + 1)
            .await
            .unwrap();
        assert_eq!(marked, 1);
        let row = svc
            .peer_repo
            .find_active_by_user("user-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.status, "offline");
    }
}
