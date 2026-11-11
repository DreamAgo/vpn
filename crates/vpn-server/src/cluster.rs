//! Device grants and gateway ownership. SQLite retains issued lease deadlines
//! across controller restarts and peer deletion.
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    net::{Ipv4Addr, SocketAddr},
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use base64::{engine::general_purpose::STANDARD, Engine};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use tokio::sync::Mutex;
use vpn_api_types::peer::{
    AdminPeerView, GatewayQualityView, ObfsMode, ObfsTransport, PeerRegisterRequest,
    PeerRegisterResponse,
};
use vpn_core::{AppError, Result};

use crate::{
    cluster_transit::{clear_controller_fence, RouteLease, Transit, TransitLink},
    middleware::audit_context,
    repositories::peer_repo_sqlite::PeerRow,
    services::{peer_service::route_policy_lock, PeerService},
};

pub const BUILTIN: &str = "local";
const LEASE_MS: i64 = 10_000;
const CLIENT_HOLD_MS: i64 = 90_000;
const READY_MS: u64 = 6_000;
const QUALITY_TTL: Duration = Duration::from_secs(60);
const DEFAULT_CONFIG_KEY: &str = "cluster_default_gateway";
const TOPOLOGY_CONFIG_KEY: &str = "cluster_managed_interfaces";

#[derive(Serialize, Deserialize)]
struct Topology {
    local_interface: String,
    remote_interfaces: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteGateway {
    pub id: String,
    name: String,
    token: zeroize::Zeroizing<String>,
    server_public_key: String,
    server_endpoint: String,
    probe_endpoint: String,
    pub transit_interface: String,
    transit_peer_ip: Ipv4Addr,
    transit_local_ip: Ipv4Addr,
    transport: Option<ObfsTransport>,
}

pub struct ClusterConfig {
    pub gateways: BTreeMap<String, RemoteGateway>,
    pub probe_endpoint: String,
    pub probe_bind: SocketAddr,
}

fn hostname_endpoint(value: &str) -> bool {
    value.rsplit_once(':').is_some_and(|(host, port)| {
        !host.is_empty()
            && host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-:[]".contains(&b))
            && port.parse::<u16>().is_ok_and(|port| port != 0)
    })
}

pub(crate) fn valid_iface(value: &str) -> bool {
    !value.is_empty()
        && value.len() < 16
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}

fn valid_secret(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn valid_public_key(value: &str) -> bool {
    STANDARD
        .decode(value)
        .is_ok_and(|bytes| bytes.len() == 32 && bytes.iter().any(|b| *b != 0))
}

impl ClusterConfig {
    pub fn from_env() -> anyhow::Result<Option<Self>> {
        let Some(path) = std::env::var_os("VPN_CLUSTER_GATEWAYS_FILE") else {
            return Ok(None);
        };
        anyhow::ensure!(cfg!(target_os = "linux"), "VPN 集群控制面仅支持 Linux");
        let probe_endpoint = std::env::var("VPN_CLUSTER_PROBE_ENDPOINT")?;
        let probe_bind = std::env::var("VPN_CLUSTER_PROBE_BIND_ADDR")?.parse()?;
        let config = Self::load(Path::new(&path), probe_endpoint, probe_bind)?;
        Ok(Some(config))
    }

    pub async fn reconcile_topology(
        config: Option<&Self>,
        pool: &SqlitePool,
        local_interface: &str,
    ) -> Result<()> {
        let previous: Option<String> =
            sqlx::query_scalar("SELECT value FROM system_config WHERE key=?")
                .bind(TOPOLOGY_CONFIG_KEY)
                .fetch_optional(pool)
                .await
                .map_err(db)?;
        let previous = previous
            .map(|json| serde_json::from_str::<Topology>(&json))
            .transpose()
            .map_err(|_| AppError::Config("集群历史互联配置无法读取".into()))?;
        let next = config.map(|config| Topology {
            local_interface: local_interface.to_owned(),
            remote_interfaces: config
                .gateways
                .values()
                .map(|gateway| (gateway.id.clone(), gateway.transit_interface.clone()))
                .collect(),
        });
        if let Some(previous) = &previous {
            let mut retired = Vec::new();
            for (id, iface) in &previous.remote_interfaces {
                if next
                    .as_ref()
                    .and_then(|topology| topology.remote_interfaces.get(id))
                    != Some(iface)
                {
                    retired.push(iface);
                }
            }
            if next
                .as_ref()
                .is_some_and(|topology| topology.local_interface != previous.local_interface)
            {
                retired.push(&previous.local_interface);
            }
            for iface in retired {
                if !valid_iface(iface)
                    || std::path::Path::new("/sys/class/net").join(iface).exists()
                {
                    return Err(AppError::Config(format!(
                        "停止已退役网关并移除互联接口 {iface} 后再启动"
                    )));
                }
            }
        }
        if let Some(topology) = next {
            let json = serde_json::to_string(&topology)
                .map_err(|error| AppError::Internal(Box::new(error)))?;
            sqlx::query("INSERT INTO system_config(key,value,updated_at) VALUES(?,?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_at=excluded.updated_at")
                .bind(TOPOLOGY_CONFIG_KEY).bind(json).bind(now_ms()).execute(pool).await.map_err(db)?;
        } else if let Some(previous) = previous {
            if !valid_iface(&previous.local_interface) {
                return Err(AppError::Config("历史内置网关接口名称无效".into()));
            }
            clear_controller_fence(&previous.local_interface).await?;
            let mut tx = pool.begin().await.map_err(db)?;
            sqlx::query("DELETE FROM cluster_remote_leases")
                .execute(&mut *tx)
                .await
                .map_err(db)?;
            sqlx::query("DELETE FROM cluster_local_leases")
                .execute(&mut *tx)
                .await
                .map_err(db)?;
            sqlx::query("UPDATE cluster_devices SET target_gateway_id=NULL,confirmed_gateway_id=NULL,confirmed_revision=NULL,revision=revision+1")
                .execute(&mut *tx).await.map_err(db)?;
            sqlx::query("DELETE FROM system_config WHERE key IN (?,?)")
                .bind(TOPOLOGY_CONFIG_KEY)
                .bind(DEFAULT_CONFIG_KEY)
                .execute(&mut *tx)
                .await
                .map_err(db)?;
            tx.commit().await.map_err(db)?;
        }
        Ok(())
    }

    pub async fn prepare_transit(
        &self,
        local_interface: &str,
        subnet: ipnet::Ipv4Net,
    ) -> Result<Transit> {
        if self.gateways.values().any(|gateway| {
            gateway.transit_interface == local_interface
                || subnet.contains(&gateway.transit_peer_ip)
                || subnet.contains(&gateway.transit_local_ip)
        }) || !valid_iface(local_interface)
        {
            return Err(AppError::Config(
                "集群互联接口及地址不得复用 VPN 内置接口及网段".into(),
            ));
        }
        let links = self
            .gateways
            .values()
            .map(|gateway| TransitLink {
                interface: gateway.transit_interface.clone(),
                local_ip: gateway.transit_local_ip,
                peer_ip: gateway.transit_peer_ip,
            })
            .collect();
        Transit::start(links, local_interface, subnet).await
    }

    fn load(path: &Path, probe_endpoint: String, probe_bind: SocketAddr) -> anyhow::Result<Self> {
        let raw = zeroize::Zeroizing::new(std::fs::read(path)?);
        let nodes: Vec<RemoteGateway> = serde_json::from_slice(&raw).map_err(|error| {
            anyhow::anyhow!(
                "集群网关配置 JSON 无效（行 {}，列 {}）",
                error.line(),
                error.column()
            )
        })?;
        anyhow::ensure!(
            (1..=32).contains(&nodes.len()),
            "集群需要配置 1 至 32 个额外网关"
        );
        anyhow::ensure!(hostname_endpoint(&probe_endpoint), "默认网关探测地址无效");
        let mut gateways = BTreeMap::new();
        let mut interfaces = BTreeSet::new();
        let mut keys = BTreeSet::new();
        let mut tokens = BTreeSet::new();
        let mut transit_ips = BTreeSet::new();
        for node in nodes {
            anyhow::ensure!(
                node.id != BUILTIN
                    && node.id.len() <= 32
                    && !node.id.is_empty()
                    && node
                        .id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-'),
                "网关 ID 无效"
            );
            anyhow::ensure!(
                !node.name.trim().is_empty() && node.name.len() <= 128,
                "网关名称无效"
            );
            anyhow::ensure!(
                valid_secret(&node.token) && tokens.insert(digest(&node.token)),
                "每台网关必须使用不同的 32 字节十六进制令牌"
            );
            anyhow::ensure!(
                valid_public_key(&node.server_public_key)
                    && keys.insert(node.server_public_key.clone()),
                "每台网关必须使用不同的 WireGuard 公钥"
            );
            anyhow::ensure!(
                valid_iface(&node.transit_interface)
                    && interfaces.insert(node.transit_interface.clone()),
                "每台网关必须使用独立的互联接口"
            );
            anyhow::ensure!(
                hostname_endpoint(&node.server_endpoint) && hostname_endpoint(&node.probe_endpoint),
                "网关 Endpoint 无效"
            );
            if let Some(transport) = &node.transport {
                anyhow::ensure!(
                    transport.protocol == "obfs-v1"
                        && hostname_endpoint(&transport.endpoint)
                        && STANDARD
                            .decode(transport.psk.as_bytes())
                            .is_ok_and(|bytes| bytes.len() == 32)
                        && (576..=9000).contains(&transport.path_mtu),
                    "网关混淆配置无效"
                );
            }
            anyhow::ensure!(
                transit_ips.insert(node.transit_peer_ip)
                    && transit_ips.insert(node.transit_local_ip)
                    && !node.transit_peer_ip.is_unspecified()
                    && !node.transit_local_ip.is_unspecified(),
                "网关互联地址必须互不相同"
            );
            let id = node.id.clone();
            anyhow::ensure!(gateways.insert(id, node).is_none(), "网关 ID 重复");
        }
        Ok(Self {
            gateways,
            probe_endpoint,
            probe_bind,
        })
    }
}

#[derive(Serialize)]
pub struct Gateway {
    id: String,
    name: String,
    probe_endpoint: String,
    ready: bool,
    is_default: bool,
}

#[derive(Serialize)]
pub struct Candidates {
    default_gateway_id: String,
    target_gateway_id: Option<String>,
    revision: i64,
    active_gateway_id: Option<String>,
    gateways: Vec<Gateway>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Device {
    supported: bool,
    granted_gateway_ids: Vec<String>,
    target_gateway_id: Option<String>,
    active_gateway_id: Option<String>,
    confirmed_gateway_id: Option<String>,
    revision: i64,
    confirmed_revision: Option<i64>,
}

#[derive(Default, sqlx::FromRow)]
struct DeviceRecord {
    target_gateway_id: Option<String>,
    revision: i64,
    confirmed_gateway_id: Option<String>,
    confirmed_revision: Option<i64>,
}

#[derive(Deserialize)]
pub struct DeviceUpdate {
    granted_gateway_ids: Vec<String>,
    target_gateway_id: Option<String>,
    revision: i64,
}

#[derive(Deserialize)]
pub struct SyncRequest {
    applied_epoch: Option<String>,
    server_public_key: String,
    vpn_subnet: String,
    transport_fingerprint: Option<String>,
}

#[derive(Serialize)]
pub struct SyncResponse {
    epoch: String,
    peers: Vec<SyncPeer>,
}

#[derive(Serialize)]
struct SyncPeer {
    public_key: String,
    vpn_ip: String,
    routed_subnets: Vec<String>,
    ttl_ms: u64,
}

#[derive(sqlx::FromRow)]
struct RemoteLease {
    vpn_ip: String,
    peer_id: String,
    public_key: String,
    gateway_id: String,
    issued_until: i64,
    client_until: i64,
    draining: bool,
}

#[derive(Default)]
struct Agent {
    issued_epoch: Option<String>,
    applied_at: Option<Instant>,
}

#[derive(Deserialize)]
pub struct GatewayQualityReport {
    rtt_ms: Option<u64>,
    loss_pct: f64,
}

impl GatewayQualityReport {
    fn validate(self) -> Result<GatewayQualityView> {
        if !(0.0..=100.0).contains(&self.loss_pct)
            || self.rtt_ms.is_some_and(|rtt| rtt > 1000)
            || (self.loss_pct == 100.0) != self.rtt_ms.is_none()
        {
            return Err(AppError::Validation("网关延迟或丢包率无效".into()));
        }
        Ok(GatewayQualityView {
            rtt_ms: self.rtt_ms,
            loss_pct: self.loss_pct,
            reported_at: now_ms(),
        })
    }
}

struct GatewayQuality {
    gateway_id: String,
    public_key: String,
    revision: i64,
    received: Instant,
    sample: GatewayQualityView,
}

#[derive(sqlx::FromRow)]
struct GatewayStatus {
    id: String,
    public_key: String,
    gateway_id: String,
    revision: Option<i64>,
    confirmed_gateway_id: Option<String>,
    confirmed_revision: Option<i64>,
}

#[derive(Default)]
struct Runtime {
    agents: HashMap<String, Agent>,
    fences: HashMap<String, Instant>,
    quality: HashMap<String, GatewayQuality>,
    resume_agents_after: Option<Instant>,
}

pub struct Cluster {
    pool: SqlitePool,
    peer: Arc<PeerService>,
    config: ClusterConfig,
    transit: Transit,
    runtime: Mutex<Runtime>,
}

fn db(error: sqlx::Error) -> AppError {
    AppError::Database(Box::new(error))
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn digest(value: &str) -> Vec<u8> {
    Sha256::digest(value.as_bytes()).to_vec()
}

pub(crate) fn transport_fingerprint(
    mode: ObfsMode,
    endpoint: &str,
    path_mtu: u16,
    key: &[u8],
) -> String {
    let mut hash = Sha256::new();
    hash.update(match mode {
        ObfsMode::LowOverheadV1 => [1],
        ObfsMode::ParanoidV1 => [2],
    });
    hash.update(path_mtu.to_be_bytes());
    hash.update(key);
    hash.update(endpoint.as_bytes());
    format!("{:x}", hash.finalize())
}

fn matches_secret(stored_hash: &[u8], secret: &str) -> bool {
    // MAC verification provides a constant-time comparison of fixed-size hashes.
    let key = b"yilian-vpn-cluster-credential-v1";
    let mut stored = Hmac::<Sha256>::new_from_slice(key).expect("valid HMAC key");
    stored.update(stored_hash);
    let mut candidate = Hmac::<Sha256>::new_from_slice(key).expect("valid HMAC key");
    candidate.update(&digest(secret));
    stored
        .verify_slice(&candidate.finalize().into_bytes())
        .is_ok()
}

fn unavailable() -> AppError {
    AppError::Conflict("目标网关暂不可用或正在等待旧网关停止转发，请重试".into())
}

fn routes(peer: &PeerRow) -> Vec<String> {
    peer.routed_subnets
        .split(',')
        .filter(|route| !route.is_empty())
        .map(str::to_owned)
        .collect()
}

impl Cluster {
    pub async fn start(
        pool: SqlitePool,
        peer: Arc<PeerService>,
        config: ClusterConfig,
        transit: Transit,
    ) -> Result<Arc<Self>> {
        if config
            .gateways
            .values()
            .any(|node| node.server_public_key == peer.server_public_key_str())
        {
            return Err(AppError::Config(
                "集群网关不得复用内置 WireGuard 接口或密钥".into(),
            ));
        }
        // On restart a live agent may still hold a lease even if wall time moved.
        let vpn_ips: Vec<String> = sqlx::query_scalar("SELECT vpn_ip FROM cluster_remote_leases")
            .fetch_all(&pool)
            .await
            .map_err(db)?;
        let quarantine_until = Instant::now() + Duration::from_millis(LEASE_MS as u64);
        let runtime = Runtime {
            resume_agents_after: (!vpn_ips.is_empty()).then_some(quarantine_until),
            fences: vpn_ips
                .into_iter()
                .map(|ip| (ip, quarantine_until))
                .collect(),
            ..Runtime::default()
        };
        let cluster = Arc::new(Self {
            pool,
            peer,
            config,
            transit,
            runtime: Mutex::new(runtime),
        });
        let default = cluster.default_gateway().await?;
        if !cluster.has_gateway(&default) {
            return Err(AppError::Config(
                "数据库中的默认网关未出现在集群配置中".into(),
            ));
        }
        if default == BUILTIN {
            // Adopt the built-in peers when first enabling cluster mode. Old
            // clients continue to renew their existing assignments by heartbeat.
            sqlx::query(r#"INSERT OR IGNORE INTO cluster_local_leases(vpn_ip,peer_id,public_key,client_until)
                SELECT p.vpn_ip,p.id,p.wg_public_key,? FROM peers p
                WHERE p.status NOT IN ('deleted','force_removed')
                AND NOT EXISTS(SELECT 1 FROM cluster_remote_leases r WHERE r.vpn_ip=p.vpn_ip)"#)
                .bind(now_ms()+CLIENT_HOLD_MS).execute(&cluster.pool).await.map_err(db)?;
        }
        cluster.refresh_transit().await?;
        Ok(cluster)
    }

    fn has_gateway(&self, id: &str) -> bool {
        id == BUILTIN || self.config.gateways.contains_key(id)
    }

    async fn default_gateway(&self) -> Result<String> {
        let value: Option<String> =
            sqlx::query_scalar("SELECT value FROM system_config WHERE key=?")
                .bind(DEFAULT_CONFIG_KEY)
                .fetch_optional(&self.pool)
                .await
                .map_err(db)?;
        Ok(value.unwrap_or_else(|| BUILTIN.to_owned()))
    }

    fn ready(runtime: &Runtime, id: &str) -> bool {
        id == BUILTIN
            || runtime
                .agents
                .get(id)
                .and_then(|agent| agent.applied_at)
                .is_some_and(|seen| seen.elapsed() < Duration::from_millis(READY_MS))
    }

    fn list_gateways(&self, runtime: &Runtime, default: &str) -> Vec<Gateway> {
        std::iter::once(Gateway {
            id: BUILTIN.to_owned(),
            name: "内置网关".to_owned(),
            probe_endpoint: self.config.probe_endpoint.clone(),
            ready: true,
            is_default: default == BUILTIN,
        })
        .chain(self.config.gateways.values().map(|node| Gateway {
            id: node.id.clone(),
            name: node.name.clone(),
            probe_endpoint: node.probe_endpoint.clone(),
            ready: Self::ready(runtime, &node.id),
            is_default: default == node.id,
        }))
        .collect()
    }

    pub async fn gateways(&self) -> Result<Vec<Gateway>> {
        let runtime = self.runtime.lock().await;
        let default = self.default_gateway().await?;
        Ok(self.list_gateways(&runtime, &default))
    }

    pub async fn set_default(&self, id: &str) -> Result<()> {
        let runtime = self.runtime.lock().await;
        if !self.has_gateway(id) || !Self::ready(&runtime, id) {
            return Err(unavailable());
        }
        let before = self.default_gateway().await?;
        let mut tx = self.pool.begin().await.map_err(db)?;
        sqlx::query("INSERT INTO system_config(key,value,updated_at) VALUES(?,?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_at=excluded.updated_at")
            .bind(DEFAULT_CONFIG_KEY).bind(id).bind(now_ms()).execute(&mut *tx).await.map_err(db)?;
        if before != id {
            // An explicit target that was only authorized by the former default
            // must not survive a default change.
            sqlx::query("UPDATE cluster_devices SET target_gateway_id=NULL WHERE target_gateway_id=? AND NOT EXISTS (SELECT 1 FROM cluster_device_grants g WHERE g.peer_id=cluster_devices.peer_id AND g.gateway_id=?)")
                .bind(&before).bind(&before).execute(&mut *tx).await.map_err(db)?;
            sqlx::query("UPDATE cluster_devices SET revision=revision+1")
                .execute(&mut *tx)
                .await
                .map_err(db)?;
        }
        audit_context::record(&mut tx, "cluster.default_gateway", before.into(), id.into()).await?;
        tx.commit().await.map_err(db)?;
        audit_context::committed();
        self.refresh_transit().await
    }

    async fn peer_by_key(&self, user: &str, key: &str) -> Result<PeerRow> {
        let peer = self
            .peer
            .peer_repo
            .find_active_by_user_and_pubkey(user, key)
            .await?
            .ok_or(AppError::PeerNotFound)?;
        if peer.status == "force_removed" {
            return Err(AppError::TokenExpired);
        }
        Ok(peer)
    }

    async fn credential(&self, key: &str) -> Result<Option<(String, Vec<u8>)>> {
        sqlx::query_as("SELECT peer_id,credential_hash FROM cluster_devices WHERE public_key=?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await
            .map_err(db)
    }

    async fn verify(&self, peer: &PeerRow, secret: Option<&str>, required: bool) -> Result<()> {
        match self.credential(&peer.wg_public_key).await? {
            Some((id, hash))
                if id == peer.id
                    && secret.is_some_and(|secret| {
                        valid_secret(secret) && matches_secret(&hash, secret)
                    }) =>
            {
                Ok(())
            }
            Some(_) => Err(AppError::NoAccess),
            None if required => Err(AppError::PeerNotFound),
            None => Ok(()),
        }
    }

    async fn lease(&self, ip: &str) -> Result<Option<RemoteLease>> {
        sqlx::query_as("SELECT vpn_ip,peer_id,public_key,gateway_id,issued_until,client_until,draining FROM cluster_remote_leases WHERE vpn_ip=?")
            .bind(ip).fetch_optional(&self.pool).await.map_err(db)
    }

    async fn enroll(&self, peer: &PeerRow, secret: &str) -> Result<()> {
        let mut tx = self.pool.begin().await.map_err(db)?;
        // The slot can be reused, but its policy revision may never go
        // backwards or a stale admin form could grant access to a new device.
        sqlx::query("DELETE FROM cluster_device_grants WHERE peer_id=? AND EXISTS(SELECT 1 FROM cluster_devices d WHERE d.peer_id=? AND d.public_key<>?)")
            .bind(&peer.id).bind(&peer.id).bind(&peer.wg_public_key).execute(&mut *tx).await.map_err(db)?;
        sqlx::query(
            r#"INSERT INTO cluster_devices(peer_id,public_key,credential_hash) VALUES(?,?,?)
            ON CONFLICT(peer_id) DO UPDATE SET public_key=excluded.public_key,
                credential_hash=excluded.credential_hash,revision=cluster_devices.revision+1,
                target_gateway_id=NULL,confirmed_gateway_id=NULL,confirmed_revision=NULL
            WHERE cluster_devices.public_key<>excluded.public_key"#,
        )
        .bind(&peer.id)
        .bind(&peer.wg_public_key)
        .bind(digest(secret))
        .execute(&mut *tx)
        .await
        .map_err(db)?;
        tx.commit().await.map_err(db)
    }

    pub async fn register(
        &self,
        user: &str,
        request: &PeerRegisterRequest,
        secret: Option<&str>,
    ) -> Result<PeerRegisterResponse> {
        let mut runtime = self.runtime.lock().await;
        if let Some(secret) = secret {
            if !valid_secret(secret) || !valid_public_key(&request.wg_public_key) {
                return Err(AppError::Validation("客户端设备凭据或公钥格式无效".into()));
            }
        }
        if let Some((id, hash)) = self.credential(&request.wg_public_key).await? {
            if !secret.is_some_and(|secret| matches_secret(&hash, secret))
                || self
                    .peer
                    .peer_repo
                    .find_by_id(&id)
                    .await?
                    .is_none_or(|peer| peer.user_id != user)
            {
                return Err(AppError::NoAccess);
            }
        }
        self.peer.ensure_user_available(user).await?;
        let default = self.default_gateway().await?;
        let mut gateway = default.clone();
        if secret.is_some() {
            if let Some(peer) = self
                .peer
                .peer_repo
                .find_active_by_user_and_pubkey(user, &request.wg_public_key)
                .await?
            {
                let device = self.read_device(&peer).await?;
                if let Some(active) = device.active_gateway_id {
                    if Self::ready(&runtime, &active) {
                        gateway = active;
                    }
                }
            }
        }
        if !Self::ready(&runtime, &gateway) {
            return Err(unavailable());
        }
        self.check_transport(&gateway, &request.capabilities)?;
        let mut response = self.peer.register_device(user, request, false).await?;
        let peer = self.peer_by_key(user, &request.wg_public_key).await?;
        if let Some(secret) = secret {
            self.enroll(&peer, secret).await?;
        }
        let _route_guard = route_policy_lock().lock_owned().await;
        let peer = self.peer_by_key(user, &request.wg_public_key).await?;
        self.assign(&mut runtime, &peer, &gateway).await?;
        self.apply_gateway(&gateway, &mut response);
        Ok(response)
    }

    async fn read_device(&self, peer: &PeerRow) -> Result<Device> {
        let row: Option<DeviceRecord> =
            sqlx::query_as("SELECT target_gateway_id,revision,confirmed_gateway_id,confirmed_revision FROM cluster_devices WHERE peer_id=? AND public_key=?")
                .bind(&peer.id).bind(&peer.wg_public_key).fetch_optional(&self.pool).await.map_err(db)?;
        let supported = row.is_some();
        let DeviceRecord {
            target_gateway_id,
            revision,
            confirmed_gateway_id,
            confirmed_revision,
        } = row.unwrap_or_default();
        let granted_gateway_ids: Vec<String> = if supported {
            sqlx::query_scalar::<_, String>(
                "SELECT gateway_id FROM cluster_device_grants WHERE peer_id=? ORDER BY gateway_id",
            )
            .bind(&peer.id)
            .fetch_all(&self.pool)
            .await
            .map_err(db)?
            .into_iter()
            .filter(|id| self.has_gateway(id))
            .collect()
        } else {
            Vec::new()
        };
        let default = self.default_gateway().await?;
        let permitted = |gateway: &str| {
            gateway == default || granted_gateway_ids.iter().any(|id| id == gateway)
        };
        let active_gateway_id = match self.lease(&peer.vpn_ip).await? {
            Some(lease)
                if lease.peer_id == peer.id
                    && lease.public_key == peer.wg_public_key
                    && !lease.draining
                    && lease.issued_until > now_ms()
                    && permitted(&lease.gateway_id)
                    && !matches!(peer.status.as_str(), "deleted" | "force_removed") =>
            {
                Some(lease.gateway_id)
            }
            Some(_) => None,
            None if permitted(BUILTIN)
                && (peer.status == "online"
                    || confirmed_gateway_id.as_deref() == Some(BUILTIN))
                && self.local_owner(peer).await? =>
            {
                Some(BUILTIN.to_owned())
            }
            None => None,
        };
        Ok(Device {
            supported,
            granted_gateway_ids,
            target_gateway_id,
            active_gateway_id,
            confirmed_gateway_id,
            revision,
            confirmed_revision,
        })
    }

    pub async fn device(&self, id: &str) -> Result<Device> {
        let _runtime = self.runtime.lock().await;
        let peer = self
            .peer
            .peer_repo
            .find_by_id(id)
            .await?
            .ok_or(AppError::PeerNotFound)?;
        self.read_device(&peer).await
    }

    /// Decorate only this page, using a single query to recheck current
    /// ownership and authorization before attaching a recent measurement.
    pub async fn fill_gateway_status(&self, peers: &mut [AdminPeerView]) -> Result<()> {
        let online: Vec<&str> = peers
            .iter()
            .filter(|peer| peer.status == "online")
            .map(|peer| peer.id.as_str())
            .collect();
        if online.is_empty() {
            return Ok(());
        }
        let runtime = self.runtime.lock().await;
        let default = self.default_gateway().await?;
        let rows = sqlx::query_as::<_, GatewayStatus>(
            r#"WITH active AS (
                SELECT p.id, p.wg_public_key AS public_key, d.revision,
                       d.confirmed_gateway_id, d.confirmed_revision,
                       CASE WHEN r.vpn_ip IS NOT NULL THEN
                           CASE WHEN r.peer_id=p.id AND r.public_key=p.wg_public_key
                                     AND r.draining=0 AND r.issued_until>?1 AND r.client_until>?1
                                THEN r.gateway_id END
                            WHEN l.client_until>?1 THEN ?2
                       END AS gateway_id
                FROM peers p
                JOIN users u ON u.id=p.user_id AND u.status='active'
                LEFT JOIN cluster_devices d ON d.peer_id=p.id AND d.public_key=p.wg_public_key
                LEFT JOIN cluster_remote_leases r ON r.vpn_ip=p.vpn_ip
                LEFT JOIN cluster_local_leases l ON l.vpn_ip=p.vpn_ip AND l.peer_id=p.id AND l.public_key=p.wg_public_key
                WHERE p.status='online' AND p.id IN (SELECT value FROM json_each(?3))
                AND NOT EXISTS (
                    SELECT 1 FROM external_identities e
                    JOIN feishu_user_states f ON f.subject=e.subject
                    WHERE e.provider='feishu' AND e.user_id=u.id AND f.blocked=1
                )
            )
            SELECT * FROM active a WHERE a.gateway_id=?4
            OR (a.revision IS NOT NULL AND EXISTS (
                SELECT 1 FROM cluster_device_grants g WHERE g.peer_id=a.id AND g.gateway_id=a.gateway_id
            ))"#,
        )
        .bind(now_ms())
        .bind(BUILTIN)
        .bind(serde_json::json!(online).to_string())
        .bind(default)
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;
        let mut rows: HashMap<_, _> = rows.into_iter().map(|row| (row.id.clone(), row)).collect();
        for peer in peers {
            let Some(row) = rows.remove(&peer.id) else {
                continue;
            };
            if row.public_key != peer.wg_public_key
                || !self.has_gateway(&row.gateway_id)
                || (row.revision.is_some()
                    && row.confirmed_gateway_id.as_deref() != Some(&row.gateway_id))
            {
                continue;
            }
            peer.gateway_quality = runtime
                .quality
                .get(&peer.id)
                .filter(|report| {
                    report.received.elapsed() < QUALITY_TTL
                        && report.public_key == row.public_key
                        && report.gateway_id == row.gateway_id
                        && Some(report.revision) == row.revision
                        && row.confirmed_revision == row.revision
                })
                .map(|report| report.sample.clone());
            peer.current_gateway_id = Some(row.gateway_id);
        }
        Ok(())
    }

    pub async fn update_device(&self, id: &str, mut input: DeviceUpdate) -> Result<Device> {
        let _runtime = self.runtime.lock().await;
        let peer = self
            .peer
            .peer_repo
            .find_by_id(id)
            .await?
            .ok_or(AppError::PeerNotFound)?;
        let before = self.read_device(&peer).await?;
        if !before.supported || before.revision != input.revision {
            return Err(AppError::Conflict(
                "设备网关配置已变更或客户端尚未登记，请刷新后重试".into(),
            ));
        }
        let default = self.default_gateway().await?;
        let granted: BTreeSet<_> = input.granted_gateway_ids.into_iter().collect();
        if granted.iter().any(|id| !self.has_gateway(id)) {
            return Err(AppError::Conflict("授权包含未知网关".into()));
        }
        if input
            .target_gateway_id
            .as_ref()
            .is_some_and(|id| id != &default && !granted.contains(id))
        {
            if input.target_gateway_id == before.target_gateway_id {
                input.target_gateway_id = None;
            } else {
                return Err(AppError::Conflict(
                    "指定网关必须是默认网关或已授权的网关".into(),
                ));
            }
        }
        let after_grants: Vec<_> = granted.into_iter().collect();
        if before.granted_gateway_ids == after_grants
            && before.target_gateway_id == input.target_gateway_id
        {
            return Ok(before);
        }
        let mut tx = self.pool.begin().await.map_err(db)?;
        sqlx::query(
            "UPDATE cluster_devices SET target_gateway_id=?,revision=revision+1 WHERE peer_id=?",
        )
        .bind(&input.target_gateway_id)
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(db)?;
        sqlx::query("DELETE FROM cluster_device_grants WHERE peer_id=?")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        for gateway in &after_grants {
            sqlx::query("INSERT INTO cluster_device_grants(peer_id,gateway_id) VALUES(?,?)")
                .bind(id)
                .bind(gateway)
                .execute(&mut *tx)
                .await
                .map_err(db)?;
        }
        sqlx::query("UPDATE cluster_remote_leases SET draining=1 WHERE peer_id=? AND gateway_id<>? AND NOT EXISTS(SELECT 1 FROM cluster_device_grants g WHERE g.peer_id=? AND g.gateway_id=cluster_remote_leases.gateway_id)")
            .bind(id).bind(&default).bind(id).execute(&mut *tx).await.map_err(db)?;
        audit_context::record(&mut tx, &format!("cluster.device.{id}"),
            serde_json::json!({"target_gateway_id":before.target_gateway_id,"granted_gateway_ids":before.granted_gateway_ids}),
            serde_json::json!({"target_gateway_id":input.target_gateway_id,"granted_gateway_ids":after_grants}),
        ).await?;
        tx.commit().await.map_err(db)?;
        audit_context::committed();
        self.refresh_transit().await?;
        self.read_device(&peer).await
    }

    pub async fn candidates(
        &self,
        user: &str,
        key: &str,
        secret: Option<&str>,
    ) -> Result<Candidates> {
        let runtime = self.runtime.lock().await;
        let peer = self.peer_by_key(user, key).await?;
        self.verify(&peer, secret, true).await?;
        let device = self.read_device(&peer).await?;
        let default = self.default_gateway().await?;
        let gateways = self
            .list_gateways(&runtime, &default)
            .into_iter()
            .filter(|gateway| {
                gateway.ready
                    && (gateway.is_default || device.granted_gateway_ids.contains(&gateway.id))
            })
            .collect();
        Ok(Candidates {
            default_gateway_id: default,
            target_gateway_id: device.target_gateway_id,
            revision: device.revision,
            active_gateway_id: device.active_gateway_id,
            gateways,
        })
    }

    pub async fn connect(
        &self,
        user: &str,
        key: &str,
        secret: Option<&str>,
        gateway: &str,
        revision: i64,
        capabilities: &[String],
    ) -> Result<PeerRegisterResponse> {
        let mut runtime = self.runtime.lock().await;
        let _route_guard = route_policy_lock().lock_owned().await;
        let peer = self.peer_by_key(user, key).await?;
        self.verify(&peer, secret, true).await?;
        self.peer.ensure_user_available(user).await?;
        let device = self.read_device(&peer).await?;
        let default = self.default_gateway().await?;
        if revision != device.revision {
            return Err(AppError::Conflict("网关配置已变更，请刷新候选节点".into()));
        }
        if gateway != default && !device.granted_gateway_ids.iter().any(|id| id == gateway) {
            return Err(AppError::NoAccess);
        }
        self.check_transport(gateway, capabilities)?;
        self.assign(&mut runtime, &peer, gateway).await?;
        let mut response = self
            .peer
            .connection_config(user, peer.vpn_ip.clone(), &routes(&peer))
            .await?;
        self.apply_gateway(gateway, &mut response);
        Ok(response)
    }

    fn check_transport(&self, gateway: &str, capabilities: &[String]) -> Result<()> {
        let required = self.config.gateways.get(gateway).map_or_else(
            || self.peer.requires_obfs(),
            |node| node.transport.is_some(),
        );
        if required
            && !capabilities
                .iter()
                .any(|capability| capability == "obfs-v1")
        {
            return Err(AppError::NoAccessReason(
                "目标网关启用了 UDP 混淆，请升级客户端".into(),
            ));
        }
        Ok(())
    }

    fn apply_gateway(&self, gateway: &str, response: &mut PeerRegisterResponse) {
        if let Some(node) = self.config.gateways.get(gateway) {
            response
                .server_public_key
                .clone_from(&node.server_public_key);
            response.server_endpoint.clone_from(&node.server_endpoint);
            response.transport.clone_from(&node.transport);
        }
    }

    async fn local_owner(&self, peer: &PeerRow) -> Result<bool> {
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM cluster_local_leases WHERE vpn_ip=? AND peer_id=? AND public_key=? AND client_until>?)")
            .bind(&peer.vpn_ip).bind(&peer.id).bind(&peer.wg_public_key).bind(now_ms())
            .fetch_one(&self.pool).await.map_err(db)
    }

    async fn assign(&self, runtime: &mut Runtime, peer: &PeerRow, gateway: &str) -> Result<()> {
        if !self.has_gateway(gateway) || !Self::ready(runtime, gateway) {
            return Err(unavailable());
        }
        runtime.quality.remove(&peer.id);
        if let Some(old) = self.lease(&peer.vpn_ip).await? {
            let same = old.peer_id == peer.id
                && old.public_key == peer.wg_public_key
                && old.gateway_id == gateway;
            if !same || old.draining {
                sqlx::query("UPDATE cluster_remote_leases SET draining=1 WHERE vpn_ip=?")
                    .bind(&peer.vpn_ip)
                    .execute(&self.pool)
                    .await
                    .map_err(db)?;
                if old.issued_until > now_ms()
                    || runtime
                        .fences
                        .get(&old.vpn_ip)
                        .is_some_and(|end| *end > Instant::now())
                {
                    return Err(unavailable());
                }
                self.refresh_transit().await?;
                sqlx::query("DELETE FROM cluster_remote_leases WHERE vpn_ip=?")
                    .bind(&peer.vpn_ip)
                    .execute(&self.pool)
                    .await
                    .map_err(db)?;
                runtime.fences.remove(&peer.vpn_ip);
            }
        }
        if gateway == BUILTIN {
            self.peer.attach_local_peer(peer).await?;
            sqlx::query("INSERT INTO cluster_local_leases(vpn_ip,peer_id,public_key,client_until) VALUES(?,?,?,?) ON CONFLICT(vpn_ip) DO UPDATE SET peer_id=excluded.peer_id,public_key=excluded.public_key,client_until=excluded.client_until")
                .bind(&peer.vpn_ip).bind(&peer.id).bind(&peer.wg_public_key).bind(now_ms()+CLIENT_HOLD_MS)
                .execute(&self.pool).await.map_err(db)?;
            self.refresh_transit().await?;
        } else {
            self.peer.detach_local_peer(peer).await?;
            sqlx::query("DELETE FROM cluster_local_leases WHERE vpn_ip=?")
                .bind(&peer.vpn_ip)
                .execute(&self.pool)
                .await
                .map_err(db)?;
            sqlx::query("INSERT INTO cluster_remote_leases(vpn_ip,peer_id,public_key,gateway_id,client_until) VALUES(?,?,?,?,?) ON CONFLICT(vpn_ip) DO UPDATE SET client_until=excluded.client_until")
                .bind(&peer.vpn_ip).bind(&peer.id).bind(&peer.wg_public_key).bind(gateway)
                .bind(now_ms()+CLIENT_HOLD_MS).execute(&self.pool).await.map_err(db)?;
        }
        sqlx::query("UPDATE cluster_devices SET confirmed_gateway_id=NULL,confirmed_revision=NULL WHERE peer_id=?")
            .bind(&peer.id).execute(&self.pool).await.map_err(db)?;
        Ok(())
    }

    pub async fn heartbeat(&self, peer: &PeerRow, secret: Option<&str>) -> Result<()> {
        let _runtime = self.runtime.lock().await;
        self.verify(peer, secret, false).await?;
        self.peer.ensure_user_available(&peer.user_id).await?;
        if self.credential(&peer.wg_public_key).await?.is_none() {
            let default = self.default_gateway().await?;
            let actual = self
                .lease(&peer.vpn_ip)
                .await?
                .filter(|lease| lease.peer_id == peer.id && lease.public_key == peer.wg_public_key)
                .map_or_else(|| BUILTIN.to_owned(), |lease| lease.gateway_id);
            if actual != default {
                return Err(AppError::NoAccessReason(
                    "默认网关已切换，请重新连接 VPN".into(),
                ));
            }
        }

        sqlx::query("UPDATE cluster_remote_leases SET client_until=? WHERE peer_id=? AND public_key=? AND draining=0")
            .bind(now_ms()+CLIENT_HOLD_MS).bind(&peer.id).bind(&peer.wg_public_key)
            .execute(&self.pool).await.map_err(db)?;
        sqlx::query(
            "UPDATE cluster_local_leases SET client_until=? WHERE peer_id=? AND public_key=?",
        )
        .bind(now_ms() + CLIENT_HOLD_MS)
        .bind(&peer.id)
        .bind(&peer.wg_public_key)
        .execute(&self.pool)
        .await
        .map_err(db)?;
        Ok(())
    }

    pub async fn confirm(
        &self,
        user: &str,
        key: &str,
        secret: Option<&str>,
        gateway: &str,
        revision: i64,
        quality: Option<GatewayQualityReport>,
    ) -> Result<()> {
        let mut runtime = self.runtime.lock().await;
        let peer = self.peer_by_key(user, key).await?;
        self.verify(&peer, secret, true).await?;
        let device = self.read_device(&peer).await?;
        let default = self.default_gateway().await?;
        let owned = self.lease(&peer.vpn_ip).await?.map_or(
            gateway == BUILTIN && self.local_owner(&peer).await?,
            |lease| {
                !lease.draining
                    && lease.gateway_id == gateway
                    && lease.peer_id == peer.id
                    && lease.public_key == key
                    && lease.issued_until > now_ms()
                    && lease.client_until > now_ms()
            },
        );
        if !owned
            || device.revision != revision
            || (gateway != default && !device.granted_gateway_ids.iter().any(|id| id == gateway))
        {
            return Err(AppError::Conflict("网关归属或配置已变更".into()));
        }
        let confirmed = device.confirmed_gateway_id.as_deref() == Some(gateway)
            && device.confirmed_revision == Some(revision);
        if let Some(report) = quality {
            if !confirmed {
                return Err(AppError::Conflict("网关尚未确认握手".into()));
            }
            runtime.quality.insert(
                peer.id,
                GatewayQuality {
                    gateway_id: gateway.to_owned(),
                    public_key: peer.wg_public_key,
                    revision,
                    received: Instant::now(),
                    sample: report.validate()?,
                },
            );
        } else if !confirmed {
            sqlx::query("UPDATE cluster_devices SET confirmed_gateway_id=?,confirmed_revision=? WHERE peer_id=?")
                .bind(gateway).bind(revision).bind(&peer.id).execute(&self.pool).await.map_err(db)?;
        }
        Ok(())
    }

    pub async fn sync(&self, id: &str, bearer: &str, input: SyncRequest) -> Result<SyncResponse> {
        let node = self
            .config
            .gateways
            .get(id)
            .ok_or(AppError::InvalidCredentials)?;
        if !valid_secret(bearer) || !matches_secret(&digest(&node.token), bearer) {
            return Err(AppError::InvalidCredentials);
        }
        let mut runtime = self.runtime.lock().await;
        let expected_transport = node.transport.as_ref().map(|transport| {
            let key = zeroize::Zeroizing::new(
                STANDARD
                    .decode(transport.psk.as_bytes())
                    .expect("validated transport PSK"),
            );
            transport_fingerprint(
                transport.mode,
                &transport.endpoint,
                transport.path_mtu,
                &key,
            )
        });
        if input.server_public_key != node.server_public_key
            || input.vpn_subnet != self.peer.vpn_subnet_cidr()
            || input.transport_fingerprint != expected_transport
        {
            runtime.agents.remove(id);
            return Err(AppError::NoAccessReason(
                "网关实际公钥、VPN 子网或混淆配置与控制面不符".into(),
            ));
        }
        if runtime
            .resume_agents_after
            .is_some_and(|deadline| deadline > Instant::now())
        {
            return Err(unavailable());
        }

        let _route_guard = route_policy_lock().lock_owned().await;
        let now = now_ms();
        let start = Instant::now();
        let default = self.default_gateway().await?;
        let rows = self.remote_routes(&default, now, Some(id)).await?;
        let mut tx = self.pool.begin().await.map_err(db)?;
        for row in &rows {
            sqlx::query("UPDATE cluster_remote_leases SET issued_until=? WHERE vpn_ip=?")
                .bind(now + LEASE_MS)
                .bind(&row.vpn_ip)
                .execute(&mut *tx)
                .await
                .map_err(db)?;
        }
        tx.commit().await.map_err(db)?;
        for row in &rows {
            runtime.fences.insert(
                row.vpn_ip.clone(),
                start + Duration::from_millis(LEASE_MS as u64),
            );
        }
        // Install central routing and its source fence before any edge can accept peers.
        self.refresh_transit().await?;
        let ttl_ms = (LEASE_MS as u64).saturating_sub(start.elapsed().as_millis() as u64);
        let agent = runtime.agents.entry(id.to_owned()).or_default();
        if agent.issued_epoch.is_some() && input.applied_epoch == agent.issued_epoch {
            agent.applied_at = Some(Instant::now());
        }
        let epoch = uuid::Uuid::now_v7().to_string();
        agent.issued_epoch = Some(epoch.clone());
        let peers = rows
            .into_iter()
            .filter(|_| ttl_ms > 0)
            .map(|row| SyncPeer {
                public_key: row.public_key,
                vpn_ip: row.vpn_ip,
                routed_subnets: row.routes,
                ttl_ms,
            })
            .collect();
        Ok(SyncResponse { epoch, peers })
    }

    async fn remote_routes(
        &self,
        default: &str,
        now: i64,
        sync_id: Option<&str>,
    ) -> Result<Vec<RouteLease>> {
        let rows: Vec<(String,String,String,String,i64)> = sqlx::query_as(
            r#"SELECT r.vpn_ip,r.public_key,r.gateway_id,p.routed_subnets,r.issued_until
               FROM cluster_remote_leases r
               JOIN peers p ON p.id=r.peer_id AND p.wg_public_key=r.public_key AND p.vpn_ip=r.vpn_ip
               JOIN users u ON u.id=p.user_id
               WHERE r.draining=0 AND r.client_until>? AND p.status NOT IN ('deleted','force_removed')
               AND u.status='active'
               AND NOT EXISTS(SELECT 1 FROM external_identities e JOIN feishu_user_states f ON f.subject=e.subject WHERE e.provider='feishu' AND e.user_id=u.id AND f.blocked=1)
               AND (r.gateway_id=? OR EXISTS(SELECT 1 FROM cluster_devices d JOIN cluster_device_grants g ON g.peer_id=d.peer_id WHERE d.peer_id=p.id AND d.public_key=p.wg_public_key AND g.gateway_id=r.gateway_id))
               AND ((? IS NULL AND r.issued_until>?) OR r.gateway_id=?) ORDER BY r.vpn_ip"#)
            .bind(now).bind(default).bind(sync_id).bind(now).bind(sync_id)
            .fetch_all(&self.pool).await.map_err(db)?;
        Ok(rows
            .into_iter()
            .filter_map(|(vpn_ip, public_key, gateway, csv, issued_until)| {
                let node = self.config.gateways.get(&gateway)?;
                Some(RouteLease {
                    vpn_ip,
                    public_key,
                    interface: node.transit_interface.clone(),
                    routes: csv
                        .split(',')
                        .filter(|route| !route.is_empty())
                        .map(str::to_owned)
                        .collect(),
                    issued_until,
                })
            })
            .collect())
    }

    async fn refresh_transit(&self) -> Result<()> {
        let default = self.default_gateway().await?;
        let now = now_ms();
        let mut rows = self.remote_routes(&default, now, None).await?;
        let local: Vec<(String, String, String, i64)> = sqlx::query_as(r#"
            SELECT l.vpn_ip,l.public_key,p.routed_subnets,l.client_until
            FROM cluster_local_leases l
            JOIN peers p ON p.id=l.peer_id AND p.wg_public_key=l.public_key AND p.vpn_ip=l.vpn_ip
            JOIN users u ON u.id=p.user_id
            WHERE l.client_until>? AND p.status NOT IN ('deleted','force_removed') AND u.status='active'
            AND NOT EXISTS(SELECT 1 FROM external_identities e JOIN feishu_user_states f ON f.subject=e.subject WHERE e.provider='feishu' AND e.user_id=u.id AND f.blocked=1)
            AND (?=? OR EXISTS(SELECT 1 FROM cluster_devices d JOIN cluster_device_grants g ON g.peer_id=d.peer_id WHERE d.peer_id=p.id AND d.public_key=p.wg_public_key AND g.gateway_id=?))"#)
            .bind(now).bind(&default).bind(BUILTIN).bind(BUILTIN).fetch_all(&self.pool).await.map_err(db)?;
        rows.extend(
            local
                .into_iter()
                .map(|(vpn_ip, public_key, csv, client_until)| RouteLease {
                    vpn_ip,
                    public_key,
                    interface: self.transit.local_interface().to_owned(),
                    routes: csv
                        .split(',')
                        .filter(|route| !route.is_empty())
                        .map(str::to_owned)
                        .collect(),
                    issued_until: client_until.min(now + LEASE_MS),
                }),
        );
        let reserved = self
            .peer
            .peer_repo
            .list_active_gateway_routes()
            .await?
            .into_iter()
            .flat_map(|(_, _, csv)| {
                csv.split(',')
                    .filter_map(|route| route.parse().ok())
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        self.transit.apply(&rows, &reserved).await
    }

    pub fn spawn(self: Arc<Self>) {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(2));
            loop {
                ticker.tick().await;
                let mut runtime = self.runtime.lock().await;
                runtime
                    .fences
                    .retain(|_, deadline| *deadline > Instant::now());
                runtime
                    .quality
                    .retain(|_, report| report.received.elapsed() < QUALITY_TTL);
                if let Err(error) = self.refresh_transit().await {
                    tracing::error!(%error, "集群互联路由同步失败");
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        repositories::{SqlitePeerRepository, SqliteSystemConfigRepository},
        services::build_peer_service,
    };
    use sqlx::sqlite::SqlitePoolOptions;

    const TOKEN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const SECRET_A: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const SECRET_B: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

    async fn fixture() -> Cluster {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        sqlx::query("INSERT INTO users(id,username,email,password_hash,role,status,must_change_password,created_at,updated_at,max_devices) VALUES('user','alice','alice@example.com','test','user','active',0,1,1,4)")
            .execute(&pool).await.unwrap();
        let peer = build_peer_service(
            SqlitePeerRepository::new(pool.clone()),
            &SqliteSystemConfigRepository::new(pool.clone()),
            "10.8.0.0/24".parse().unwrap(),
            "local.example.com:51820".into(),
        )
        .await
        .unwrap();
        let peer = peer.with_cluster_routes(true);
        let gateway = RemoteGateway {
            id: "edge".into(),
            name: "Edge".into(),
            token: zeroize::Zeroizing::new(TOKEN.into()),
            server_public_key: STANDARD.encode([42; 32]),
            server_endpoint: "edge.example.com:51820".into(),
            probe_endpoint: "edge.example.com:47359".into(),
            transit_interface: "transit0".into(),
            transit_local_ip: "172.30.0.1".parse().unwrap(),
            transit_peer_ip: "172.30.0.2".parse().unwrap(),
            transport: None,
        };
        let config = ClusterConfig {
            gateways: [(gateway.id.clone(), gateway)].into(),
            probe_endpoint: "local.example.com:47359".into(),
            probe_bind: "127.0.0.1:0".parse().unwrap(),
        };
        Arc::try_unwrap(
            Cluster::start(pool, Arc::new(peer), config, Transit::mock())
                .await
                .unwrap(),
        )
        .ok()
        .unwrap()
    }

    fn registration(name: &str, key: u8) -> PeerRegisterRequest {
        PeerRegisterRequest {
            wg_public_key: STANDARD.encode([key; 32]),
            device_name: name.into(),
            os_info: None,
            client_version: None,
            capabilities: vec![],
        }
    }

    fn request(applied_epoch: Option<String>) -> SyncRequest {
        SyncRequest {
            applied_epoch,
            server_public_key: STANDARD.encode([42; 32]),
            vpn_subnet: "10.8.0.0/24".into(),
            transport_fingerprint: None,
        }
    }

    async fn ready(cluster: &Cluster) {
        let first = cluster.sync("edge", TOKEN, request(None)).await.unwrap();
        assert!(first.peers.is_empty());
        cluster
            .sync("edge", TOKEN, request(Some(first.epoch)))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn grants_are_per_device_and_switch_keeps_ip_after_the_old_lease_drains() {
        let cluster = fixture().await;
        let a = registration("Laptop A", 7);
        let b = registration("Laptop B", 8);
        let initial = cluster.register("user", &a, Some(SECRET_A)).await.unwrap();
        cluster.register("user", &b, Some(SECRET_B)).await.unwrap();
        ready(&cluster).await;
        let peer = cluster.peer_by_key("user", &a.wg_public_key).await.unwrap();

        let candidates = cluster
            .candidates("user", &a.wg_public_key, Some(SECRET_A))
            .await
            .unwrap();
        assert_eq!(candidates.gateways.len(), 1);
        assert_eq!(candidates.gateways[0].id, BUILTIN);
        assert!(matches!(
            cluster
                .update_device(
                    &peer.id,
                    DeviceUpdate {
                        granted_gateway_ids: vec![],
                        target_gateway_id: Some("edge".into()),
                        revision: 1,
                    }
                )
                .await,
            Err(AppError::Conflict(_))
        ));
        let policy = cluster
            .update_device(
                &peer.id,
                DeviceUpdate {
                    granted_gateway_ids: vec!["edge".into()],
                    target_gateway_id: Some("edge".into()),
                    revision: 1,
                },
            )
            .await
            .unwrap();
        assert_eq!(policy.revision, 2);

        assert!(matches!(
            cluster
                .connect("user", &a.wg_public_key, Some(SECRET_B), "edge", 2, &[])
                .await,
            Err(AppError::NoAccess)
        ));
        assert!(matches!(
            cluster
                .connect("user", &b.wg_public_key, Some(SECRET_B), "edge", 1, &[])
                .await,
            Err(AppError::NoAccess)
        ));
        assert!(matches!(
            cluster
                .connect("user", &a.wg_public_key, Some(SECRET_A), "edge", 1, &[])
                .await,
            Err(AppError::Conflict(_))
        ));
        let moved = cluster
            .connect("user", &a.wg_public_key, Some(SECRET_A), "edge", 2, &[])
            .await
            .unwrap();
        assert_eq!(moved.vpn_ip, initial.vpn_ip);
        assert_eq!(moved.server_endpoint, "edge.example.com:51820");
        assert!(!cluster.local_owner(&peer).await.unwrap());
        let issued = cluster.sync("edge", TOKEN, request(None)).await.unwrap();
        assert_eq!(issued.peers.len(), 1);
        assert_eq!(issued.peers[0].vpn_ip, initial.vpn_ip);
        assert!(issued.peers[0].ttl_ms <= 10_000);
        cluster
            .confirm("user", &a.wg_public_key, Some(SECRET_A), "edge", 2, None)
            .await
            .unwrap();
        assert_eq!(
            cluster
                .device(&peer.id)
                .await
                .unwrap()
                .confirmed_gateway_id
                .as_deref(),
            Some("edge")
        );

        let revoked = cluster
            .update_device(
                &peer.id,
                DeviceUpdate {
                    granted_gateway_ids: vec![],
                    target_gateway_id: Some("edge".into()),
                    revision: 2,
                },
            )
            .await
            .unwrap();
        assert!(revoked.target_gateway_id.is_none());
        assert!(revoked.active_gateway_id.is_none());
        let snapshot = cluster
            .sync("edge", TOKEN, request(Some(issued.epoch)))
            .await
            .unwrap();
        assert!(snapshot.peers.is_empty());
        assert!(matches!(
            cluster
                .connect("user", &a.wg_public_key, Some(SECRET_A), BUILTIN, 3, &[])
                .await,
            Err(AppError::Conflict(_))
        ));
        sqlx::query("UPDATE cluster_remote_leases SET issued_until=0")
            .execute(&cluster.pool)
            .await
            .unwrap();
        cluster.runtime.lock().await.fences.clear();
        let returned = cluster
            .connect("user", &a.wg_public_key, Some(SECRET_A), BUILTIN, 3, &[])
            .await
            .unwrap();
        assert_eq!(returned.vpn_ip, initial.vpn_ip);
        assert!(cluster.local_owner(&peer).await.unwrap());
    }

    async fn listed_device(cluster: &Cluster, id: &str) -> AdminPeerView {
        let mut page = cluster
            .peer
            .list_admin_peers(&vpn_api_types::peer::AdminPeerQuery {
                page: None,
                page_size: None,
                search: None,
                status: None,
            })
            .await
            .unwrap();
        cluster.fill_gateway_status(&mut page.items).await.unwrap();
        page.items.into_iter().find(|peer| peer.id == id).unwrap()
    }

    #[tokio::test]
    async fn admin_list_shows_only_fresh_quality_for_the_confirmed_current_gateway() {
        let cluster = fixture().await;
        let a = registration("Laptop", 7);
        cluster.register("user", &a, Some(SECRET_A)).await.unwrap();
        let peer = cluster.peer_by_key("user", &a.wg_public_key).await.unwrap();
        cluster
            .peer
            .peer_repo
            .touch_heartbeat_by_id(&peer.id, None, Some(72), Some(40.0), now_ms())
            .await
            .unwrap();

        let online = listed_device(&cluster, &peer.id).await;
        assert!(online.current_gateway_id.is_none());
        assert_eq!(online.rtt_ms, Some(72));
        assert_eq!(online.loss_pct, Some(40.0));
        cluster
            .confirm("user", &a.wg_public_key, Some(SECRET_A), BUILTIN, 1, None)
            .await
            .unwrap();
        let connected = listed_device(&cluster, &peer.id).await;
        assert_eq!(connected.current_gateway_id.as_deref(), Some(BUILTIN));
        assert!(connected.gateway_quality.is_none());

        cluster
            .confirm(
                "user",
                &a.wg_public_key,
                Some(SECRET_A),
                BUILTIN,
                1,
                Some(GatewayQualityReport {
                    rtt_ms: Some(23),
                    loss_pct: 5.0,
                }),
            )
            .await
            .unwrap();
        let measured = listed_device(&cluster, &peer.id)
            .await
            .gateway_quality
            .unwrap();
        assert_eq!(measured.rtt_ms, Some(23));
        assert_eq!(measured.loss_pct, 5.0);
        cluster
            .runtime
            .lock()
            .await
            .quality
            .get_mut(&peer.id)
            .unwrap()
            .received = Instant::now() - QUALITY_TTL;
        assert!(listed_device(&cluster, &peer.id)
            .await
            .gateway_quality
            .is_none());

        cluster
            .confirm(
                "user",
                &a.wg_public_key,
                Some(SECRET_A),
                BUILTIN,
                1,
                Some(GatewayQualityReport {
                    rtt_ms: Some(8),
                    loss_pct: 0.0,
                }),
            )
            .await
            .unwrap();
        ready(&cluster).await;
        cluster
            .update_device(
                &peer.id,
                DeviceUpdate {
                    granted_gateway_ids: vec!["edge".into()],
                    target_gateway_id: None,
                    revision: 1,
                },
            )
            .await
            .unwrap();
        let revised = listed_device(&cluster, &peer.id).await;
        assert_eq!(revised.current_gateway_id.as_deref(), Some(BUILTIN));
        assert!(revised.gateway_quality.is_none());

        cluster
            .connect("user", &a.wg_public_key, Some(SECRET_A), "edge", 2, &[])
            .await
            .unwrap();
        cluster.sync("edge", TOKEN, request(None)).await.unwrap();
        assert!(listed_device(&cluster, &peer.id)
            .await
            .current_gateway_id
            .is_none());
        assert!(matches!(
            cluster
                .confirm(
                    "user",
                    &a.wg_public_key,
                    Some(SECRET_A),
                    BUILTIN,
                    2,
                    Some(GatewayQualityReport {
                        rtt_ms: Some(1),
                        loss_pct: 0.0,
                    })
                )
                .await,
            Err(AppError::Conflict(_))
        ));
        let report = || GatewayQualityReport {
            rtt_ms: None,
            loss_pct: 100.0,
        };
        assert!(matches!(
            cluster
                .confirm(
                    "user",
                    &a.wg_public_key,
                    Some(SECRET_A),
                    "edge",
                    2,
                    Some(report())
                )
                .await,
            Err(AppError::Conflict(_))
        ));
        cluster
            .confirm("user", &a.wg_public_key, Some(SECRET_A), "edge", 2, None)
            .await
            .unwrap();
        sqlx::query("UPDATE cluster_remote_leases SET client_until=0 WHERE peer_id=?")
            .bind(&peer.id)
            .execute(&cluster.pool)
            .await
            .unwrap();
        assert!(matches!(
            cluster
                .confirm(
                    "user",
                    &a.wg_public_key,
                    Some(SECRET_A),
                    "edge",
                    2,
                    Some(report())
                )
                .await,
            Err(AppError::Conflict(_))
        ));
        cluster.heartbeat(&peer, Some(SECRET_A)).await.unwrap();
        cluster
            .confirm(
                "user",
                &a.wg_public_key,
                Some(SECRET_A),
                "edge",
                2,
                Some(GatewayQualityReport {
                    rtt_ms: None,
                    loss_pct: 100.0,
                }),
            )
            .await
            .unwrap();
        let edge = listed_device(&cluster, &peer.id).await;
        assert_eq!(edge.current_gateway_id.as_deref(), Some("edge"));
        let quality = edge.gateway_quality.unwrap();
        assert_eq!(quality.rtt_ms, None);
        assert_eq!(quality.loss_pct, 100.0);

        cluster
            .update_device(
                &peer.id,
                DeviceUpdate {
                    granted_gateway_ids: vec![],
                    target_gateway_id: None,
                    revision: 2,
                },
            )
            .await
            .unwrap();
        let revoked = listed_device(&cluster, &peer.id).await;
        assert!(revoked.current_gateway_id.is_none());
        assert!(revoked.gateway_quality.is_none());
    }

    #[tokio::test]
    async fn a_reused_device_name_does_not_inherit_extra_access() {
        let cluster = fixture().await;
        let a = registration("Laptop", 7);
        let b = registration("Laptop", 8);
        cluster.register("user", &a, Some(SECRET_A)).await.unwrap();
        let peer = cluster.peer_by_key("user", &a.wg_public_key).await.unwrap();
        cluster
            .update_device(
                &peer.id,
                DeviceUpdate {
                    granted_gateway_ids: vec!["edge".into()],
                    target_gateway_id: Some("edge".into()),
                    revision: 1,
                },
            )
            .await
            .unwrap();
        cluster.register("user", &b, Some(SECRET_B)).await.unwrap();
        let next = cluster
            .candidates("user", &b.wg_public_key, Some(SECRET_B))
            .await
            .unwrap();
        assert_eq!(next.gateways.len(), 1);
        assert_eq!(next.target_gateway_id, None);
    }

    #[tokio::test]
    async fn stale_policy_cannot_authorize_a_different_device_that_reuses_an_offline_slot() {
        let cluster = fixture().await;
        sqlx::query("UPDATE users SET max_devices=1")
            .execute(&cluster.pool)
            .await
            .unwrap();
        let a = registration("Original", 7);
        cluster.register("user", &a, Some(SECRET_A)).await.unwrap();
        let first = cluster.peer_by_key("user", &a.wg_public_key).await.unwrap();
        let stale = cluster.device(&first.id).await.unwrap().revision;

        let b = registration("Replacement", 8);
        cluster.register("user", &b, Some(SECRET_B)).await.unwrap();
        let replacement = cluster.peer_by_key("user", &b.wg_public_key).await.unwrap();
        assert_eq!(first.id, replacement.id);
        assert!(matches!(
            cluster
                .update_device(
                    &first.id,
                    DeviceUpdate {
                        granted_gateway_ids: vec!["edge".into()],
                        target_gateway_id: Some("edge".into()),
                        revision: stale,
                    }
                )
                .await,
            Err(AppError::Conflict(_))
        ));
    }

    #[tokio::test]
    async fn wrong_gateway_wireguard_identity_clears_readiness() {
        let cluster = fixture().await;
        ready(&cluster).await;
        assert!(cluster
            .gateways()
            .await
            .unwrap()
            .into_iter()
            .any(|gateway| gateway.id == "edge" && gateway.ready));
        let mut wrong = request(None);
        wrong.server_public_key = STANDARD.encode([7; 32]);
        assert!(matches!(
            cluster.sync("edge", TOKEN, wrong).await,
            Err(AppError::NoAccessReason(_))
        ));
        assert!(cluster
            .gateways()
            .await
            .unwrap()
            .into_iter()
            .any(|gateway| gateway.id == "edge" && !gateway.ready));
    }
}
