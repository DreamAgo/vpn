//! 持久化网络参数：数据库优先，环境变量只在整组配置不存在时初始化一次。

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use vpn_api_types::system::{
    obfs_transport_safe_mtu, NetworkMtuMode, NetworkSettings, DEFAULT_TUN_MTU, MAX_TUN_MTU,
    MIN_TUN_MTU,
};
use vpn_core::{AppError, Result};

use crate::{
    config::{NetworkSettingsSeed, ObfsConfig},
    repositories::SqliteSystemConfigRepository,
};

pub const KEY_NETWORK_SETTINGS: &str = "network_settings_v1";
const NETWORK_SETTINGS_VERSION: u8 = 1;

#[derive(Debug, Serialize, Deserialize)]
struct PersistedNetworkSettings {
    version: u8,
    mode: NetworkMtuMode,
    default_mtu: u16,
    min_mtu: u16,
    max_mtu: u16,
}

impl From<NetworkSettings> for PersistedNetworkSettings {
    fn from(value: NetworkSettings) -> Self {
        Self {
            version: NETWORK_SETTINGS_VERSION,
            mode: value.mode,
            default_mtu: value.default_mtu,
            min_mtu: value.min_mtu,
            max_mtu: value.max_mtu,
        }
    }
}

impl TryFrom<PersistedNetworkSettings> for NetworkSettings {
    type Error = AppError;

    fn try_from(value: PersistedNetworkSettings) -> Result<Self> {
        if value.version != NETWORK_SETTINGS_VERSION {
            return Err(AppError::Config(format!(
                "{KEY_NETWORK_SETTINGS} 版本不支持：{}",
                value.version
            )));
        }
        let settings = Self {
            mode: value.mode,
            default_mtu: value.default_mtu,
            min_mtu: value.min_mtu,
            max_mtu: value.max_mtu,
        };
        settings
            .validate()
            .map_err(|error| AppError::Config(format!("{KEY_NETWORK_SETTINGS} 损坏：{error}")))?;
        Ok(settings)
    }
}

#[derive(Clone)]
pub struct NetworkSettingsService {
    repo: SqliteSystemConfigRepository,
    settings: Arc<RwLock<NetworkSettings>>,
    obfs_constraint: Option<ObfsMtuConstraint>,
}

#[derive(Debug, Clone, Copy)]
struct ObfsMtuConstraint {
    mode: vpn_api_types::peer::ObfsMode,
    path_mtu: u16,
    safe_mtu: u16,
}

impl From<&ObfsConfig> for ObfsMtuConstraint {
    fn from(config: &ObfsConfig) -> Self {
        Self {
            mode: config.mode,
            path_mtu: config.path_mtu,
            safe_mtu: obfs_transport_safe_mtu(config.mode, config.path_mtu),
        }
    }
}

impl NetworkSettingsService {
    pub async fn load_or_seed(
        repo: SqliteSystemConfigRepository,
        seed: &NetworkSettingsSeed,
        obfs: Option<&ObfsConfig>,
    ) -> Result<Self> {
        let obfs_constraint = obfs.map(ObfsMtuConstraint::from);
        let raw = match repo.get(KEY_NETWORK_SETTINGS).await? {
            Some(raw) => raw,
            None => {
                let settings = parse_seed(seed)?;
                validate_obfs_constraint(&settings, obfs_constraint).map_err(AppError::Config)?;
                let raw = serialize(&settings)?;
                repo.set_if_absent(KEY_NETWORK_SETTINGS, &raw).await?;
                repo.get(KEY_NETWORK_SETTINGS).await?.ok_or_else(|| {
                    AppError::Config(format!("{KEY_NETWORK_SETTINGS} 初始化后读取失败"))
                })?
            }
        };
        let settings = deserialize(&raw)?;
        validate_obfs_constraint(&settings, obfs_constraint).map_err(AppError::Config)?;
        Ok(Self {
            repo,
            settings: Arc::new(RwLock::new(settings)),
            obfs_constraint,
        })
    }

    pub async fn settings(&self) -> NetworkSettings {
        self.settings.read().await.clone()
    }

    pub fn shared_settings(&self) -> Arc<RwLock<NetworkSettings>> {
        self.settings.clone()
    }

    /// 校验成功后先持久化整组 JSON，再更新运行时快照。
    pub async fn update(&self, settings: NetworkSettings) -> Result<NetworkSettings> {
        settings.validate().map_err(AppError::Validation)?;
        validate_obfs_constraint(&settings, self.obfs_constraint).map_err(AppError::Validation)?;
        let mut current = self.settings.write().await;
        self.repo
            .set(KEY_NETWORK_SETTINGS, &serialize(&settings)?)
            .await?;
        *current = settings.clone();
        Ok(settings)
    }
}

fn serialize(settings: &NetworkSettings) -> Result<String> {
    serde_json::to_string(&PersistedNetworkSettings::from(settings.clone()))
        .map_err(|error| AppError::Internal(Box::new(error)))
}

fn deserialize(raw: &str) -> Result<NetworkSettings> {
    let persisted: PersistedNetworkSettings = serde_json::from_str(raw)
        .map_err(|error| AppError::Config(format!("{KEY_NETWORK_SETTINGS} JSON 损坏：{error}")))?;
    persisted.try_into()
}

fn parse_seed(seed: &NetworkSettingsSeed) -> Result<NetworkSettings> {
    let mode = match seed.mode.as_deref().unwrap_or("fixed") {
        "fixed" => NetworkMtuMode::Fixed,
        "auto" => NetworkMtuMode::Auto,
        value => {
            return Err(AppError::Config(format!(
                "VPN_TUN_MTU_MODE 必须是 fixed 或 auto，当前为 {value}"
            )))
        }
    };
    let settings = NetworkSettings {
        mode,
        default_mtu: parse_seed_mtu(
            "VPN_TUN_MTU_DEFAULT",
            seed.default_mtu.as_deref(),
            DEFAULT_TUN_MTU,
        )?,
        min_mtu: parse_seed_mtu("VPN_TUN_MTU_MIN", seed.min_mtu.as_deref(), MIN_TUN_MTU)?,
        max_mtu: parse_seed_mtu("VPN_TUN_MTU_MAX", seed.max_mtu.as_deref(), MAX_TUN_MTU)?,
    };
    settings.validate().map_err(|_| {
        AppError::Config(format!(
            "VPN_TUN_MTU_* 首次初始化配置非法：VPN_TUN_MTU_MIN={}、VPN_TUN_MTU_DEFAULT={}、VPN_TUN_MTU_MAX={}，违反 1280 <= VPN_TUN_MTU_MIN <= VPN_TUN_MTU_DEFAULT <= VPN_TUN_MTU_MAX <= 1420",
            settings.min_mtu, settings.default_mtu, settings.max_mtu
        ))
    })?;
    Ok(settings)
}

fn validate_obfs_constraint(
    settings: &NetworkSettings,
    constraint: Option<ObfsMtuConstraint>,
) -> std::result::Result<(), String> {
    let Some(constraint) = constraint else {
        return Ok(());
    };
    let (field, value) = match settings.mode {
        NetworkMtuMode::Fixed => ("default_mtu", settings.default_mtu),
        NetworkMtuMode::Auto => ("min_mtu", settings.min_mtu),
    };
    if value <= constraint.safe_mtu {
        return Ok(());
    }
    Err(format!(
        "当前混淆传输 mode={:?}、path_mtu={} 的安全内层 MTU 上限为 {}，{}={} 超过该上限",
        constraint.mode, constraint.path_mtu, constraint.safe_mtu, field, value
    ))
}

fn parse_seed_mtu(name: &str, raw: Option<&str>, default: u16) -> Result<u16> {
    match raw {
        Some(value) => value
            .parse::<u16>()
            .map_err(|_| AppError::Config(format!("{name} 必须是整数，当前为 {value}"))),
        None => Ok(default),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::str::FromStr;
    use zeroize::Zeroizing;

    async fn repo() -> SqliteSystemConfigRepository {
        let url = format!(
            "sqlite:file:network_settings_test_{}?mode=memory&cache=private",
            uuid::Uuid::new_v4()
        );
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(SqliteConnectOptions::from_str(&url).unwrap())
            .await
            .unwrap();
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        SqliteSystemConfigRepository::new(pool)
    }

    fn obfs(mode: vpn_api_types::peer::ObfsMode, path_mtu: u16) -> ObfsConfig {
        ObfsConfig {
            bind_addr: "0.0.0.0:47358".into(),
            public_endpoint: "vpn.example.com:47358".into(),
            mode,
            path_mtu,
            psk: Zeroizing::new([7; 32]),
            max_sessions: 16,
            new_sessions_per_ip_per_minute: 20,
            session_idle_secs: 180,
        }
    }

    #[tokio::test]
    async fn environment_seed_is_written_once_and_db_wins_afterward() {
        let repo = repo().await;
        let first = NetworkSettingsSeed {
            mode: Some("auto".into()),
            default_mtu: Some("1340".into()),
            min_mtu: Some("1280".into()),
            max_mtu: Some("1400".into()),
        };
        let service = NetworkSettingsService::load_or_seed(repo.clone(), &first, None)
            .await
            .unwrap();
        assert_eq!(service.settings().await.default_mtu, 1340);

        let changed_environment = NetworkSettingsSeed {
            mode: Some("not-valid".into()),
            default_mtu: Some("not-a-number".into()),
            ..NetworkSettingsSeed::default()
        };
        let restarted = NetworkSettingsService::load_or_seed(repo, &changed_environment, None)
            .await
            .unwrap();
        assert_eq!(restarted.settings().await, service.settings().await);
    }

    #[tokio::test]
    async fn invalid_seed_fails_without_writing_partial_configuration() {
        let repo = repo().await;
        let seed = NetworkSettingsSeed {
            min_mtu: Some("1400".into()),
            default_mtu: Some("1360".into()),
            ..NetworkSettingsSeed::default()
        };
        let error = NetworkSettingsService::load_or_seed(repo.clone(), &seed, None)
            .await
            .err()
            .expect("非法 seed 应拒绝启动");
        let message = error.to_string();
        assert!(message.contains("VPN_TUN_MTU_MIN=1400"));
        assert!(message.contains("VPN_TUN_MTU_DEFAULT=1360"));
        assert!(message.contains("VPN_TUN_MTU_MAX=1420"));
        assert!(message
            .contains("1280 <= VPN_TUN_MTU_MIN <= VPN_TUN_MTU_DEFAULT <= VPN_TUN_MTU_MAX <= 1420"));
        assert!(repo.get(KEY_NETWORK_SETTINGS).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn corrupt_stored_configuration_never_falls_back_to_environment() {
        let repo = repo().await;
        repo.set(KEY_NETWORK_SETTINGS, "{bad json").await.unwrap();
        assert!(
            NetworkSettingsService::load_or_seed(repo, &NetworkSettingsSeed::default(), None)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn invalid_update_keeps_previous_value() {
        let repo = repo().await;
        let service =
            NetworkSettingsService::load_or_seed(repo, &NetworkSettingsSeed::default(), None)
                .await
                .unwrap();
        let before = service.settings().await;
        let invalid = NetworkSettings {
            min_mtu: 1400,
            default_mtu: 1360,
            ..before.clone()
        };
        assert!(service.update(invalid).await.is_err());
        assert_eq!(service.settings().await, before);
    }

    #[tokio::test]
    async fn fixed_seed_rejects_default_above_obfs_safe_mtu() {
        let repo = repo().await;
        let seed = NetworkSettingsSeed {
            default_mtu: Some("1400".into()),
            max_mtu: Some("1420".into()),
            ..NetworkSettingsSeed::default()
        };
        let obfs = obfs(vpn_api_types::peer::ObfsMode::ParanoidV1, 1500);
        let error = NetworkSettingsService::load_or_seed(repo.clone(), &seed, Some(&obfs))
            .await
            .err()
            .expect("fixed default 超过安全上限应拒绝启动");
        assert!(error.to_string().contains("default_mtu=1400"));
        assert!(error.to_string().contains("安全内层 MTU 上限为 1392"));
        assert!(repo.get(KEY_NETWORK_SETTINGS).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn obfs_path_below_global_minimum_is_rejected_at_startup() {
        let repo = repo().await;
        let obfs = obfs(vpn_api_types::peer::ObfsMode::LowOverheadV1, 1200);
        let error = NetworkSettingsService::load_or_seed(
            repo,
            &NetworkSettingsSeed::default(),
            Some(&obfs),
        )
        .await
        .err()
        .expect("低于全局最小 MTU 的路径应拒绝启动");
        assert!(error.to_string().contains("安全内层 MTU 上限为 1136"));
    }

    #[tokio::test]
    async fn auto_update_rejects_min_above_obfs_safe_mtu_without_changing_value() {
        let repo = repo().await;
        let obfs = obfs(vpn_api_types::peer::ObfsMode::ParanoidV1, 1500);
        let service = NetworkSettingsService::load_or_seed(
            repo,
            &NetworkSettingsSeed::default(),
            Some(&obfs),
        )
        .await
        .unwrap();
        let before = service.settings().await;
        let invalid = NetworkSettings {
            mode: NetworkMtuMode::Auto,
            min_mtu: 1400,
            default_mtu: 1400,
            max_mtu: 1420,
        };
        let error = service.update(invalid).await.unwrap_err();
        assert!(error.to_string().contains("min_mtu=1400"));
        assert_eq!(service.settings().await, before);
    }
}
