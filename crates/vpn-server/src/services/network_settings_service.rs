//! 版本化数据面配置：环境变量仅初始化，数据库中的 desired 配置优先。

use crate::{
    config::{DataPlaneSettingsSeed, NetworkSettingsSeed},
    repositories::{SqlitePeerRepository, SqliteSystemConfigRepository},
    services::peer_service::{normalize_subnets, route_policy_lock, KEY_SERVER_ROUTES},
};
use serde::{Deserialize, Serialize};
use std::{
    net::Ipv4Addr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use base64::Engine;
use tokio::sync::RwLock;
use vpn_api_types::{
    peer::ObfsMode,
    system::{
        normalize_local_route_bypass, ClientDnsMode, DataPlaneSettings, DnsForwardRule,
        DnsNetworkSettings, DnsStaticRecord, LocalRouteBypassRule, NetworkMtuMode, NetworkSettings,
        NetworkSettingsView, ObfsNetworkSettings, VpnBaseSettings, DEFAULT_TUN_MTU, MAX_TUN_MTU,
        MIN_TUN_MTU,
    },
};
use vpn_core::{AppError, Result};

pub const KEY_NETWORK_SETTINGS: &str = "network_settings_v1";
const KEY_DATA_PLANE_SETTINGS_V2: &str = "network_settings_v2";
pub const KEY_DATA_PLANE_SETTINGS: &str = "network_settings_v3";
pub const KEY_LOCAL_ROUTE_BYPASS: &str = "local_route_bypass_v1";
const VERSION: u8 = 3;

#[derive(Debug, Serialize, Deserialize)]
struct PersistedSettings {
    version: u8,
    settings: DataPlaneSettings,
}
#[derive(Debug, Deserialize)]
struct PersistedV1 {
    version: u8,
    mode: NetworkMtuMode,
    default_mtu: u16,
    min_mtu: u16,
    max_mtu: u16,
}

#[derive(Clone)]
pub struct NetworkSettingsService {
    repo: SqliteSystemConfigRepository,
    peer_repo: SqlitePeerRepository,
    desired: Arc<RwLock<DataPlaneSettings>>,
    applied: DataPlaneSettings,
    mtu: Arc<RwLock<NetworkSettings>>,
    dns: Arc<RwLock<DnsNetworkSettings>>,
    local_route_bypass: Arc<RwLock<Vec<LocalRouteBypassRule>>>,
    psk_configured: bool,
    https_enabled: bool,
    approval_enabled: bool,
    registration_blocked: Arc<AtomicBool>,
}

impl NetworkSettingsService {
    pub async fn load_or_seed(
        repo: SqliteSystemConfigRepository,
        peer_repo: SqlitePeerRepository,
        seed: &DataPlaneSettingsSeed,
        mtu_seed: &NetworkSettingsSeed,
        https_enabled: bool,
        approval_enabled: bool,
    ) -> Result<Self> {
        let psk_configured = seed
            .obfs_psk
            .as_deref()
            .is_some_and(|value| valid_psk(value.as_str()));
        let raw = match repo.get(KEY_DATA_PLANE_SETTINGS).await? {
            Some(raw) => raw,
            None => {
                let settings = match repo.get(KEY_DATA_PLANE_SETTINGS_V2).await? {
                    Some(raw) => deserialize_v2(&raw)?,
                    None => {
                        let mtu = match repo.get(KEY_NETWORK_SETTINGS).await? {
                            Some(raw) => deserialize_v1(&raw)?,
                            None => parse_mtu_seed(mtu_seed)?,
                        };
                        parse_seed(seed, mtu)?
                    }
                };
                validate_prerequisites(&settings, https_enabled, psk_configured, approval_enabled)
                    .map_err(AppError::Config)?;
                let raw = serialize(&settings)?;
                repo.set_if_absent(KEY_DATA_PLANE_SETTINGS, &raw).await?;
                repo.get(KEY_DATA_PLANE_SETTINGS).await?.ok_or_else(|| {
                    AppError::Config(format!("{KEY_DATA_PLANE_SETTINGS} 初始化后读取失败"))
                })?
            }
        };
        let settings = deserialize(&raw)?;
        validate_prerequisites(&settings, https_enabled, psk_configured, approval_enabled)
            .map_err(AppError::Config)?;
        validate_peer_ips(&peer_repo, &settings.vpn.vpn_subnet).await?;
        load_or_seed_server_routes(&repo, seed.server_routes.as_deref()).await?;
        repo.set_if_absent(KEY_LOCAL_ROUTE_BYPASS, "[]").await?;
        let raw_rules = repo.get(KEY_LOCAL_ROUTE_BYPASS).await?.ok_or_else(|| {
            AppError::Config(format!("{KEY_LOCAL_ROUTE_BYPASS} 初始化后读取失败"))
        })?;
        let rules: Vec<LocalRouteBypassRule> =
            serde_json::from_str(&raw_rules).map_err(|error| {
                AppError::Config(format!("{KEY_LOCAL_ROUTE_BYPASS} JSON 损坏：{error}"))
            })?;
        let rules = normalize_local_route_bypass(&rules)
            .map_err(|error| AppError::Config(format!("{KEY_LOCAL_ROUTE_BYPASS} 损坏：{error}")))?;
        Ok(Self {
            repo,
            peer_repo,
            desired: Arc::new(RwLock::new(settings.clone())),
            applied: settings.clone(),
            mtu: Arc::new(RwLock::new(settings.mtu.clone())),
            dns: Arc::new(RwLock::new(settings.dns.clone())),
            local_route_bypass: Arc::new(RwLock::new(rules)),
            psk_configured,
            https_enabled,
            approval_enabled,
            registration_blocked: Arc::new(AtomicBool::new(false)),
        })
    }

    pub async fn desired(&self) -> DataPlaneSettings {
        self.desired.read().await.clone()
    }
    pub fn applied(&self) -> &DataPlaneSettings {
        &self.applied
    }
    pub fn shared_settings(&self) -> Arc<RwLock<NetworkSettings>> {
        self.mtu.clone()
    }
    pub fn shared_dns_settings(&self) -> Arc<RwLock<DnsNetworkSettings>> {
        self.dns.clone()
    }
    pub fn shared_local_route_bypass(&self) -> Arc<RwLock<Vec<LocalRouteBypassRule>>> {
        self.local_route_bypass.clone()
    }
    pub fn registration_gate(&self) -> Arc<AtomicBool> {
        self.registration_blocked.clone()
    }

    pub async fn view(&self, server_routes: Vec<String>) -> NetworkSettingsView {
        let desired = self.desired().await;
        let mut applied = self.applied.clone();
        // MTU 对新连接/重连即时生效，不属于待重启字段；返回当前实际下发值。
        applied.mtu = self.mtu.read().await.clone();
        applied.dns = self.dns.read().await.clone();
        NetworkSettingsView {
            restart_required: restart_fields(&self.applied) != restart_fields(&desired),
            applied,
            desired,
            server_routes,
            local_route_bypass: self.local_route_bypass.read().await.clone(),
            psk_configured: self.psk_configured,
        }
    }

    pub async fn server_routes(&self) -> Result<Vec<String>> {
        let raw = self
            .repo
            .get(KEY_SERVER_ROUTES)
            .await?
            .ok_or_else(|| AppError::Config(format!("{KEY_SERVER_ROUTES} 未初始化")))?;
        normalize_subnets(&split_routes(&raw))
            .map_err(|error| AppError::Config(format!("{KEY_SERVER_ROUTES} 损坏：{error}")))
    }

    pub async fn update(
        &self,
        desired: DataPlaneSettings,
        server_routes: &[String],
    ) -> Result<Vec<String>> {
        let lock = route_policy_lock();
        let _guard = lock.lock().await;
        self.update_locked(desired, server_routes, None).await
    }

    pub(crate) async fn update_locked(
        &self,
        mut desired: DataPlaneSettings,
        server_routes: &[String],
        local_route_bypass: Option<&[LocalRouteBypassRule]>,
    ) -> Result<Vec<String>> {
        desired.dns = desired.dns.normalized().map_err(AppError::Validation)?;
        desired.validate().map_err(AppError::Validation)?;
        validate_prerequisites(
            &desired,
            self.https_enabled,
            self.psk_configured,
            self.approval_enabled,
        )
        .map_err(AppError::Validation)?;
        // MTU 会立即下发给重连客户端；待重启的混淆参数尚未生效时，也必须满足当前
        // applied transport 的安全上限。
        let mut applied_with_new_mtu = self.applied.clone();
        applied_with_new_mtu.mtu = desired.mtu.clone();
        applied_with_new_mtu.dns = desired.dns.clone();
        validate_prerequisites(
            &applied_with_new_mtu,
            self.https_enabled,
            self.psk_configured,
            self.approval_enabled,
        )
        .map_err(AppError::Validation)?;
        let routes = normalize_subnets(server_routes)?;
        let rules = match local_route_bypass {
            Some(rules) => normalize_local_route_bypass(rules).map_err(AppError::Validation)?,
            None => self.local_route_bypass.read().await.clone(),
        };
        // 串行化完整校验、事务提交和内存快照切换，避免并发保存导致 DB/内存倒序。
        let mut current = self.desired.write().await;
        if desired.vpn.vpn_subnet != current.vpn.vpn_subnet && self.peer_repo.count_all().await? > 0
        {
            return Err(AppError::Validation(
                "已有 Peer（包括已删除记录），必须彻底清理节点后才能修改 vpn_subnet".into(),
            ));
        }
        let raw = serialize(&desired)?;
        let raw_rules =
            serde_json::to_string(&rules).map_err(|error| AppError::Internal(Box::new(error)))?;
        self.repo
            .set_many(&[
                (KEY_DATA_PLANE_SETTINGS, &raw),
                (KEY_SERVER_ROUTES, &routes.join(",")),
                (KEY_LOCAL_ROUTE_BYPASS, &raw_rules),
            ])
            .await?;
        *self.mtu.write().await = desired.mtu.clone();
        *self.dns.write().await = desired.dns.clone();
        *self.local_route_bypass.write().await = rules;
        self.registration_blocked.store(
            desired.vpn.vpn_subnet != self.applied.vpn.vpn_subnet,
            Ordering::Release,
        );
        *current = desired;
        Ok(routes)
    }
}

fn restart_fields(settings: &DataPlaneSettings) -> (&VpnBaseSettings, &ObfsNetworkSettings) {
    (&settings.vpn, &settings.obfs)
}
fn serialize(settings: &DataPlaneSettings) -> Result<String> {
    serde_json::to_string(&PersistedSettings {
        version: VERSION,
        settings: settings.clone(),
    })
    .map_err(|error| AppError::Internal(Box::new(error)))
}
fn deserialize(raw: &str) -> Result<DataPlaneSettings> {
    let persisted: PersistedSettings = serde_json::from_str(raw).map_err(|error| {
        AppError::Config(format!("{KEY_DATA_PLANE_SETTINGS} JSON 损坏：{error}"))
    })?;
    if persisted.version != VERSION {
        return Err(AppError::Config(format!(
            "{KEY_DATA_PLANE_SETTINGS} 版本不支持：{}",
            persisted.version
        )));
    }
    persisted
        .settings
        .validate()
        .map_err(|error| AppError::Config(format!("{KEY_DATA_PLANE_SETTINGS} 损坏：{error}")))?;
    let mut settings = persisted.settings;
    settings.dns = settings.dns.normalized().map_err(|error| {
        AppError::Config(format!("{KEY_DATA_PLANE_SETTINGS} DNS 损坏：{error}"))
    })?;
    Ok(settings)
}
fn deserialize_v2(raw: &str) -> Result<DataPlaneSettings> {
    let persisted: PersistedSettings = serde_json::from_str(raw).map_err(|error| {
        AppError::Config(format!("{KEY_DATA_PLANE_SETTINGS_V2} JSON 损坏：{error}"))
    })?;
    if persisted.version != 2 {
        return Err(AppError::Config(format!(
            "{KEY_DATA_PLANE_SETTINGS_V2} 版本不支持：{}",
            persisted.version
        )));
    }
    persisted
        .settings
        .validate()
        .map_err(|error| AppError::Config(format!("{KEY_DATA_PLANE_SETTINGS_V2} 损坏：{error}")))?;
    let mut settings = persisted.settings;
    settings.dns = settings.dns.normalized().map_err(|error| {
        AppError::Config(format!("{KEY_DATA_PLANE_SETTINGS_V2} DNS 损坏：{error}"))
    })?;
    Ok(settings)
}
fn deserialize_v1(raw: &str) -> Result<NetworkSettings> {
    let value: PersistedV1 = serde_json::from_str(raw)
        .map_err(|error| AppError::Config(format!("{KEY_NETWORK_SETTINGS} JSON 损坏：{error}")))?;
    if value.version != 1 {
        return Err(AppError::Config(format!(
            "{KEY_NETWORK_SETTINGS} 版本不支持：{}",
            value.version
        )));
    }
    let mtu = NetworkSettings {
        mode: value.mode,
        default_mtu: value.default_mtu,
        min_mtu: value.min_mtu,
        max_mtu: value.max_mtu,
    };
    mtu.validate()
        .map_err(|error| AppError::Config(format!("{KEY_NETWORK_SETTINGS} 损坏：{error}")))?;
    Ok(mtu)
}

fn parse_seed(seed: &DataPlaneSettingsSeed, mtu: NetworkSettings) -> Result<DataPlaneSettings> {
    let enabled = match seed.obfs_enabled.as_deref().unwrap_or("false") {
        "1" | "true" | "TRUE" | "yes" | "YES" => true,
        "0" | "false" | "FALSE" | "no" | "NO" => false,
        value => return Err(AppError::Config(format!("VPN_OBFS_ENABLED 非法：{value}"))),
    };
    let mode = match seed.obfs_mode.as_deref().unwrap_or("low-overhead-v1") {
        "low-overhead-v1" => ObfsMode::LowOverheadV1,
        "paranoid-v1" => ObfsMode::ParanoidV1,
        value => return Err(AppError::Config(format!("VPN_OBFS_MODE 不支持：{value}"))),
    };
    let settings = DataPlaneSettings {
        vpn: VpnBaseSettings {
            vpn_subnet: seed
                .vpn_subnet
                .clone()
                .unwrap_or_else(|| "10.8.0.0/24".into()),
            vpn_listen_port: parse_u16("VPN_LISTEN_PORT", seed.vpn_listen_port.as_deref(), 51820)?,
            vpn_endpoint: seed
                .vpn_endpoint
                .clone()
                .unwrap_or_else(|| "127.0.0.1:51820".into()),
            wg_backend: seed.wg_backend.clone().unwrap_or_else(|| "noop".into()),
            wg_interface: seed.wg_interface.clone().unwrap_or_else(|| "wg0".into()),
        },
        obfs: ObfsNetworkSettings {
            enabled,
            mode,
            bind_addr: seed
                .obfs_bind_addr
                .clone()
                .unwrap_or_else(|| "0.0.0.0:47358".into()),
            public_endpoint: seed
                .obfs_endpoint
                .clone()
                .unwrap_or_else(|| "127.0.0.1:47358".into()),
            path_mtu: parse_u16("VPN_OBFS_PATH_MTU", seed.obfs_path_mtu.as_deref(), 1500)?,
        },
        mtu,
        dns: parse_dns_seed(seed)?,
    };
    settings.validate().map_err(AppError::Config)?;
    Ok(settings)
}

fn parse_dns_seed(seed: &DataPlaneSettingsSeed) -> Result<DnsNetworkSettings> {
    let mode = match seed.dns_mode.as_deref().unwrap_or("disabled") {
        "disabled" => ClientDnsMode::Disabled,
        "global" | "split" => ClientDnsMode::Global,
        value => return Err(AppError::Config(format!("VPN_DNS_MODE 非法：{value}"))),
    };
    let default_upstreams = split_csv(seed.dns_default_upstreams.as_deref());
    let forward_rules: Vec<DnsForwardRule> =
        parse_json_seed("VPN_DNS_FORWARD_RULES", seed.dns_forward_rules.as_deref())?;
    let static_records: Vec<DnsStaticRecord> =
        parse_json_seed("VPN_DNS_STATIC_RECORDS", seed.dns_static_records.as_deref())?;
    let settings = DnsNetworkSettings {
        mode,
        default_upstreams,
        forward_rules,
        static_records,
    };
    settings.normalized().map_err(AppError::Config)
}

fn split_csv(raw: Option<&str>) -> Vec<String> {
    raw.unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}

fn parse_json_seed<T: serde::de::DeserializeOwned>(
    name: &str,
    raw: Option<&str>,
) -> Result<Vec<T>> {
    match raw.filter(|value| !value.trim().is_empty()) {
        Some(value) => serde_json::from_str(value)
            .map_err(|error| AppError::Config(format!("{name} JSON 非法：{error}"))),
        None => Ok(Vec::new()),
    }
}

fn parse_mtu_seed(seed: &NetworkSettingsSeed) -> Result<NetworkSettings> {
    let mode = match seed.mode.as_deref().unwrap_or("fixed") {
        "fixed" => NetworkMtuMode::Fixed,
        "auto" => NetworkMtuMode::Auto,
        value => return Err(AppError::Config(format!("VPN_TUN_MTU_MODE 非法：{value}"))),
    };
    let mtu = NetworkSettings {
        mode,
        default_mtu: parse_u16(
            "VPN_TUN_MTU_DEFAULT",
            seed.default_mtu.as_deref(),
            DEFAULT_TUN_MTU,
        )?,
        min_mtu: parse_u16("VPN_TUN_MTU_MIN", seed.min_mtu.as_deref(), MIN_TUN_MTU)?,
        max_mtu: parse_u16("VPN_TUN_MTU_MAX", seed.max_mtu.as_deref(), MAX_TUN_MTU)?,
    };
    mtu.validate().map_err(|_| AppError::Config(format!(
        "VPN_TUN_MTU_* 首次初始化配置非法：VPN_TUN_MTU_MIN={}、VPN_TUN_MTU_DEFAULT={}、VPN_TUN_MTU_MAX={}，违反 1280 <= VPN_TUN_MTU_MIN <= VPN_TUN_MTU_DEFAULT <= VPN_TUN_MTU_MAX <= 1420",
        mtu.min_mtu, mtu.default_mtu, mtu.max_mtu)))?;
    Ok(mtu)
}
fn parse_u16(name: &str, raw: Option<&str>, default: u16) -> Result<u16> {
    raw.map_or(Ok(default), |value| {
        value
            .parse()
            .map_err(|_| AppError::Config(format!("{name} 必须是整数，当前为 {value}")))
    })
}
fn validate_prerequisites(
    settings: &DataPlaneSettings,
    https_enabled: bool,
    psk_configured: bool,
    approval_enabled: bool,
) -> std::result::Result<(), String> {
    settings.validate()?;
    let subnet: ipnet::Ipv4Net = settings
        .vpn
        .vpn_subnet
        .parse()
        .map_err(|_| "vpn_subnet 非法".to_string())?;
    let gateway = subnet
        .hosts()
        .next()
        .ok_or_else(|| "VPN 子网没有可用网关地址".to_string())?;
    for upstream in settings.dns.default_upstreams.iter().chain(
        settings
            .dns
            .forward_rules
            .iter()
            .flat_map(|rule| rule.upstreams.iter()),
    ) {
        let address: std::net::SocketAddr = upstream
            .parse()
            .map_err(|_| format!("DNS 上游非法：{upstream}"))?;
        if address.ip() == std::net::IpAddr::V4(gateway) {
            return Err(format!("DNS 上游不能指回 VPN 网关自身：{upstream}"));
        }
    }
    if settings.obfs.enabled && !https_enabled {
        return Err("启用 UDP 混淆时必须启用 HTTPS，禁止通过 HTTP 下发 PSK".into());
    }
    if settings.dns.mode != ClientDnsMode::Disabled && settings.vpn.wg_backend == "noop" {
        return Err("启用内置 DNS 时 wg_backend 不能为 noop".into());
    }
    if settings.obfs.enabled && !psk_configured {
        return Err("启用 UDP 混淆时必须通过 VPN_OBFS_PSK 配置秘密".into());
    }
    if approval_enabled && settings.vpn.wg_backend != "kernel" {
        return Err("启用飞书审批网络授权时 wg_backend 必须为 kernel".into());
    }
    Ok(())
}

async fn load_or_seed_server_routes(
    repo: &SqliteSystemConfigRepository,
    seed: Option<&str>,
) -> Result<Vec<String>> {
    match repo.get(KEY_SERVER_ROUTES).await? {
        Some(raw) => normalize_subnets(&split_routes(&raw))
            .map_err(|error| AppError::Config(format!("{KEY_SERVER_ROUTES} 损坏：{error}"))),
        None => {
            let routes = normalize_subnets(&split_routes(seed.unwrap_or_default()))
                .map_err(|error| AppError::Config(format!("VPN_SERVER_ROUTES 非法：{error}")))?;
            repo.set_if_absent(KEY_SERVER_ROUTES, &routes.join(","))
                .await?;
            let stored = repo
                .get(KEY_SERVER_ROUTES)
                .await?
                .ok_or_else(|| AppError::Config(format!("{KEY_SERVER_ROUTES} 初始化后读取失败")))?;
            normalize_subnets(&split_routes(&stored))
                .map_err(|error| AppError::Config(format!("{KEY_SERVER_ROUTES} 损坏：{error}")))
        }
    }
}

fn split_routes(raw: &str) -> Vec<String> {
    raw.split(',').map(str::to_owned).collect()
}

fn valid_psk(value: &str) -> bool {
    base64::engine::general_purpose::STANDARD
        .decode(value.trim())
        .is_ok_and(|decoded| decoded.len() == 32)
}
async fn validate_peer_ips(repo: &SqlitePeerRepository, subnet: &str) -> Result<()> {
    let subnet: ipnet::Ipv4Net = subnet
        .parse()
        .map_err(|_| AppError::Config("vpn_subnet 非法".into()))?;
    for raw in repo.list_all_vpn_ips().await? {
        let ip: Ipv4Addr = raw
            .parse()
            .map_err(|_| AppError::Config(format!("peers 表包含非法 vpn_ip={raw}，拒绝启动")))?;
        if !subnet.contains(&ip) {
            return Err(AppError::Config(format!(
                "Peer IP {ip} 不在当前 vpn_subnet {subnet} 内，拒绝启动"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::str::FromStr;
    async fn repos() -> (SqliteSystemConfigRepository, SqlitePeerRepository) {
        let url = format!(
            "sqlite:file:network_v2_{}?mode=memory&cache=private",
            uuid::Uuid::new_v4()
        );
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(SqliteConnectOptions::from_str(&url).unwrap())
            .await
            .unwrap();
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        (
            SqliteSystemConfigRepository::new(pool.clone()),
            SqlitePeerRepository::new(pool),
        )
    }
    fn seed() -> DataPlaneSettingsSeed {
        DataPlaneSettingsSeed {
            vpn_subnet: Some("10.8.0.0/24".into()),
            vpn_listen_port: Some("51820".into()),
            vpn_endpoint: Some("vpn.example.com:51820".into()),
            wg_backend: Some("kernel".into()),
            wg_interface: Some("wg0".into()),
            obfs_enabled: Some("false".into()),
            obfs_mode: Some("low-overhead-v1".into()),
            obfs_bind_addr: Some("0.0.0.0:47358".into()),
            obfs_endpoint: Some("vpn.example.com:47358".into()),
            obfs_path_mtu: Some("1500".into()),
            server_routes: Some(String::new()),
            dns_mode: None,
            dns_default_upstreams: None,
            dns_forward_rules: None,
            dns_static_records: None,
            obfs_psk: None,
        }
    }
    async fn insert_deleted_peer(repo: &SqliteSystemConfigRepository, vpn_ip: &str) {
        sqlx::query("INSERT INTO users (id, username, email, password_hash, role, status, created_at, updated_at) VALUES ('u', 'u', 'u@example.com', 'x', 'user', 'active', 1, 1)")
            .execute(repo.pool()).await.unwrap();
        sqlx::query("INSERT INTO peers (id, user_id, device_name, wg_public_key, vpn_ip, status, created_at, updated_at) VALUES ('p', 'u', 'd', 'key', ?1, 'deleted', 1, 1)")
            .bind(vpn_ip).execute(repo.pool()).await.unwrap();
    }
    #[tokio::test]
    async fn migrates_v1_mtu_and_v2_db_ignores_later_bad_environment() {
        let (repo, peers) = repos().await;
        repo.set(
            KEY_NETWORK_SETTINGS,
            r#"{"version":1,"mode":"auto","default_mtu":1340,"min_mtu":1280,"max_mtu":1400}"#,
        )
        .await
        .unwrap();
        let mut initial_seed = seed();
        initial_seed.server_routes = Some("192.168.10.0/24".into());
        let service = NetworkSettingsService::load_or_seed(
            repo.clone(),
            peers.clone(),
            &initial_seed,
            &NetworkSettingsSeed::default(),
            false,
            false,
        )
        .await
        .unwrap();
        assert_eq!(service.desired().await.mtu.default_mtu, 1340);
        assert_eq!(
            service.server_routes().await.unwrap(),
            vec!["192.168.10.0/24"]
        );
        let mut bad = seed();
        bad.vpn_subnet = Some("bad".into());
        bad.obfs_mode = Some("bad".into());
        bad.server_routes = Some("bad-route".into());
        let restarted = NetworkSettingsService::load_or_seed(
            repo,
            peers,
            &bad,
            &NetworkSettingsSeed {
                mode: Some("bad".into()),
                ..Default::default()
            },
            false,
            false,
        )
        .await
        .unwrap();
        assert_eq!(restarted.desired().await, service.desired().await);
        assert_eq!(
            restarted.server_routes().await.unwrap(),
            vec!["192.168.10.0/24"]
        );
    }

    #[tokio::test]
    async fn migrates_v2_to_v3_with_dns_disabled() {
        let (repo, peers) = repos().await;
        repo.set(
            KEY_DATA_PLANE_SETTINGS_V2,
            r#"{"version":2,"settings":{"vpn":{"vpn_subnet":"10.8.0.0/24","vpn_listen_port":51820,"vpn_endpoint":"vpn.example.com:51820","wg_backend":"kernel","wg_interface":"wg0"},"obfs":{"enabled":false,"mode":"low-overhead-v1","bind_addr":"0.0.0.0:47358","public_endpoint":"vpn.example.com:47358","path_mtu":1500},"mtu":{"mode":"fixed","default_mtu":1360,"min_mtu":1280,"max_mtu":1420}}}"#,
        )
        .await
        .unwrap();
        let service = NetworkSettingsService::load_or_seed(
            repo.clone(),
            peers,
            &seed(),
            &NetworkSettingsSeed::default(),
            false,
            false,
        )
        .await
        .unwrap();
        assert_eq!(service.desired().await.dns.mode, ClientDnsMode::Disabled);
        assert!(repo.get(KEY_DATA_PLANE_SETTINGS).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn legacy_split_database_loads_as_global_and_save_drops_domains() {
        let (repo, peers) = repos().await;
        let settings = parse_seed(&seed(), NetworkSettings::default()).unwrap();
        let mut stored = serde_json::to_value(PersistedSettings {
            version: 3,
            settings,
        })
        .unwrap();
        stored["settings"]["dns"] = serde_json::json!({
            "mode": "split", "split_domains": ["invalid legacy domain"],
            "default_upstreams": ["223.5.5.5:53"]
        });
        repo.set(KEY_DATA_PLANE_SETTINGS, &stored.to_string())
            .await
            .unwrap();
        let service = NetworkSettingsService::load_or_seed(
            repo.clone(),
            peers,
            &seed(),
            &NetworkSettingsSeed::default(),
            false,
            false,
        )
        .await
        .unwrap();
        let desired = service.desired().await;
        assert_eq!(desired.dns.mode, ClientDnsMode::Global);
        assert_eq!(
            service.shared_dns_settings().read().await.mode,
            ClientDnsMode::Global
        );
        service.update(desired, &[]).await.unwrap();
        let saved: serde_json::Value =
            serde_json::from_str(&repo.get(KEY_DATA_PLANE_SETTINGS).await.unwrap().unwrap())
                .unwrap();
        assert_eq!(saved["settings"]["dns"]["mode"], "global");
        assert!(saved["settings"]["dns"].get("split_domains").is_none());
    }

    #[test]
    fn legacy_split_environment_seed_is_global_and_unknown_mode_fails() {
        let mut seed = seed();
        seed.dns_mode = Some("split".into());
        seed.dns_default_upstreams = Some("223.5.5.5:53".into());
        let settings = parse_dns_seed(&seed).unwrap();
        assert_eq!(settings.mode, ClientDnsMode::Global);
        assert_eq!(serde_json::to_value(settings).unwrap()["mode"], "global");
        seed.dns_mode = Some("unknown".into());
        assert!(parse_dns_seed(&seed).is_err());
    }

    #[tokio::test]
    async fn dns_update_is_hot_and_updates_shared_snapshot() {
        let (repo, peers) = repos().await;
        let service = NetworkSettingsService::load_or_seed(
            repo,
            peers,
            &seed(),
            &NetworkSettingsSeed::default(),
            false,
            false,
        )
        .await
        .unwrap();
        let mut desired = service.desired().await;
        desired.dns = DnsNetworkSettings {
            mode: ClientDnsMode::Global,
            default_upstreams: vec!["223.5.5.5:53".into()],
            ..Default::default()
        };
        service.update(desired, &[]).await.unwrap();
        assert_eq!(
            service.shared_dns_settings().read().await.mode,
            ClientDnsMode::Global
        );
        assert!(!service.view(vec![]).await.restart_required);
    }

    fn bypass_rules() -> Vec<LocalRouteBypassRule> {
        vec![LocalRouteBypassRule {
            local_subnets: vec!["192.168.187.4/24".into()],
            excluded_routes: vec!["192.168.188.111/24".into()],
        }]
    }

    #[tokio::test]
    async fn bypass_rules_are_hot_persist_across_restart_and_can_be_cleared() {
        let (repo, peers) = repos().await;
        let service = NetworkSettingsService::load_or_seed(
            repo.clone(),
            peers.clone(),
            &seed(),
            &NetworkSettingsSeed::default(),
            false,
            false,
        )
        .await
        .unwrap();
        let shared = service.shared_local_route_bypass();
        assert!(shared.read().await.is_empty());
        let rules = bypass_rules();
        service
            .update_locked(service.desired().await, &[], Some(&rules))
            .await
            .unwrap();
        let expected = normalize_local_route_bypass(&rules).unwrap();
        assert_eq!(*shared.read().await, expected);
        assert!(!service.view(vec![]).await.restart_required);
        // 老管理端保存其他参数时没有字段，必须保留规则。
        service.update(service.desired().await, &[]).await.unwrap();
        assert_eq!(*shared.read().await, expected);
        let restarted = NetworkSettingsService::load_or_seed(
            repo.clone(),
            peers.clone(),
            &seed(),
            &NetworkSettingsSeed::default(),
            false,
            false,
        )
        .await
        .unwrap();
        assert_eq!(restarted.view(vec![]).await.local_route_bypass, expected);
        restarted
            .update_locked(restarted.desired().await, &[], Some(&[]))
            .await
            .unwrap();
        assert!(restarted
            .shared_local_route_bypass()
            .read()
            .await
            .is_empty());
        let restarted_again = NetworkSettingsService::load_or_seed(
            repo,
            peers,
            &seed(),
            &NetworkSettingsSeed::default(),
            false,
            false,
        )
        .await
        .unwrap();
        assert!(restarted_again
            .view(vec![])
            .await
            .local_route_bypass
            .is_empty());
    }

    #[tokio::test]
    async fn failed_bypass_save_rolls_back_data_plane_routes_and_shared_snapshot() {
        let (repo, peers) = repos().await;
        let service = NetworkSettingsService::load_or_seed(
            repo.clone(),
            peers,
            &seed(),
            &NetworkSettingsSeed::default(),
            false,
            false,
        )
        .await
        .unwrap();
        let original = service.desired().await;
        let original_raw = repo.get(KEY_DATA_PLANE_SETTINGS).await.unwrap();
        sqlx::query("CREATE TRIGGER reject_bypass_update BEFORE UPDATE ON system_config WHEN NEW.key = 'local_route_bypass_v1' BEGIN SELECT RAISE(FAIL, 'test policy storage failure'); END")
            .execute(repo.pool()).await.unwrap();
        let mut desired = original.clone();
        desired.mtu.default_mtu = 1340;
        assert!(service
            .update_locked(desired, &["10.0.0.0/8".into()], Some(&bypass_rules()))
            .await
            .is_err());
        assert_eq!(service.desired().await, original);
        assert_eq!(
            repo.get(KEY_DATA_PLANE_SETTINGS).await.unwrap(),
            original_raw
        );
        assert!(service.server_routes().await.unwrap().is_empty());
        assert!(service.shared_local_route_bypass().read().await.is_empty());
        assert_eq!(
            service.shared_settings().read().await.default_mtu,
            original.mtu.default_mtu
        );
    }

    #[tokio::test]
    async fn corrupt_persisted_bypass_rules_refuse_startup() {
        for raw in [
            "{bad",
            r#"[{"local_subnets":["192.168.187.0/24"],"excluded_routes":["0.0.0.0/0"]}]"#,
        ] {
            let (repo, peers) = repos().await;
            repo.set(KEY_LOCAL_ROUTE_BYPASS, raw).await.unwrap();
            let result = NetworkSettingsService::load_or_seed(
                repo,
                peers,
                &seed(),
                &NetworkSettingsSeed::default(),
                false,
                false,
            )
            .await;
            assert!(result
                .err()
                .unwrap()
                .to_string()
                .contains(KEY_LOCAL_ROUTE_BYPASS));
        }
    }
    #[tokio::test]
    async fn wide_lan_route_and_empty_peer_subnet_change_are_allowed() {
        let (repo, peers) = repos().await;
        let service = NetworkSettingsService::load_or_seed(
            repo,
            peers,
            &seed(),
            &NetworkSettingsSeed::default(),
            false,
            false,
        )
        .await
        .unwrap();
        let mut desired = service.desired().await;
        desired.vpn.vpn_subnet = "10.9.0.0/24".into();
        let routes = service
            .update(desired, &["10.0.0.0/8".into()])
            .await
            .unwrap();
        assert_eq!(routes, vec!["10.0.0.0/8"]);
        assert!(service.view(routes).await.restart_required);
        assert!(service.registration_gate().load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn approval_mode_rejects_non_kernel_desired_backend() {
        let (repo, peers) = repos().await;
        let service = NetworkSettingsService::load_or_seed(
            repo,
            peers,
            &seed(),
            &NetworkSettingsSeed::default(),
            false,
            true,
        )
        .await
        .unwrap();
        let mut desired = service.desired().await;
        desired.vpn.wg_backend = "auto".into();
        assert!(service.update(desired, &[]).await.is_err());
    }
    #[tokio::test]
    async fn corrupt_v2_refuses_startup() {
        let (repo, peers) = repos().await;
        repo.set(KEY_DATA_PLANE_SETTINGS, "{bad").await.unwrap();
        assert!(NetworkSettingsService::load_or_seed(
            repo,
            peers,
            &seed(),
            &NetworkSettingsSeed::default(),
            false,
            false
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn deleted_peer_blocks_subnet_change_and_outside_ip_blocks_restart() {
        let (repo, peers) = repos().await;
        let service = NetworkSettingsService::load_or_seed(
            repo.clone(),
            peers.clone(),
            &seed(),
            &NetworkSettingsSeed::default(),
            false,
            false,
        )
        .await
        .unwrap();
        insert_deleted_peer(&repo, "10.8.0.2").await;
        let mut desired = service.desired().await;
        desired.vpn.vpn_subnet = "10.9.0.0/24".into();
        assert!(service
            .update(desired, &[])
            .await
            .unwrap_err()
            .to_string()
            .contains("彻底清理"));

        let raw = serialize(&DataPlaneSettings {
            vpn: VpnBaseSettings {
                vpn_subnet: "10.9.0.0/24".into(),
                ..service.desired().await.vpn
            },
            ..service.desired().await
        })
        .unwrap();
        repo.set(KEY_DATA_PLANE_SETTINGS, &raw).await.unwrap();
        let error = NetworkSettingsService::load_or_seed(
            repo,
            peers,
            &seed(),
            &NetworkSettingsSeed::default(),
            false,
            false,
        )
        .await
        .err()
        .unwrap();
        assert!(error.to_string().contains("不在当前 vpn_subnet"));
    }
}
