//! SQLite 授权事实源到 nftables 短租约快照的同步服务。

use std::{net::Ipv4Addr, sync::Arc, time::Duration};

use chrono::Utc;
use ipnet::Ipv4Net;
use sqlx::SqlitePool;
use tokio::sync::Mutex;
use vpn_core::{AppError, Result};
use vpn_wireguard::{AclLease, NftAclController};

use super::PeerService;

pub const ACL_REFRESH_INTERVAL: Duration = Duration::from_secs(20);
pub const ACL_LEASE_CAP_MS: u64 = 75_000;

#[derive(Clone)]
pub struct NetworkAclService {
    pool: SqlitePool,
    peer_service: Arc<PeerService>,
    controller: NftAclController,
    vpn_subnet: Ipv4Net,
    refresh_lock: Arc<Mutex<()>>,
}

impl NetworkAclService {
    pub fn new(
        pool: SqlitePool,
        peer_service: Arc<PeerService>,
        iface: &str,
        vpn_subnet: Ipv4Net,
    ) -> Result<Self> {
        Ok(Self {
            pool,
            peer_service,
            controller: NftAclController::new(iface, vpn_subnet)?,
            vpn_subnet,
            refresh_lock: Arc::new(Mutex::new(())),
        })
    }

    /// kernel 后端启动必须先成功建立 fail-closed 快照，再对外提供服务。
    pub async fn start(&self) -> Result<()> {
        self.controller.verify_available().await?;
        self.refresh().await
    }

    pub async fn refresh(&self) -> Result<()> {
        let _guard = self.refresh_lock.lock().await;
        let now = Utc::now().timestamp_millis();
        let server_routes = self.peer_service.server_routes().await;
        let (mut leases, site_sources) =
            build_acl_snapshot(&self.pool, &server_routes, self.vpn_subnet, now).await?;
        // 查询快照本身会消耗时间；在真正下发前扣除耗时，避免授权越过数据库中的
        // 独占 expires_at。过期元素直接丢弃。
        let elapsed_ms = Utc::now().timestamp_millis().saturating_sub(now) as u64;
        for lease in &mut leases {
            lease.timeout_ms = lease.timeout_ms.saturating_sub(elapsed_ms);
        }
        leases.retain(|lease| lease.timeout_ms > 0);
        self.controller.apply(&leases, &site_sources).await
    }

    pub fn spawn(self: Arc<Self>) {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(ACL_REFRESH_INTERVAL);
            ticker.tick().await;
            loop {
                ticker.tick().await;
                if let Err(error) = self.refresh().await {
                    // 不清空旧表；其中租约会在上限内自然失效，故运行时错误仍 fail-closed。
                    tracing::error!(error = %error, "刷新 nft ACL 失败，将重试；旧授权会按租约自动失效");
                }
            }
        });
    }
}

async fn build_acl_snapshot(
    pool: &SqlitePool,
    server_routes: &[String],
    vpn_subnet: Ipv4Net,
    now: i64,
) -> Result<(Vec<AclLease>, Vec<Ipv4Net>)> {
    let peers: Vec<(String, String)> = sqlx::query_as(
        r#"SELECT p.vpn_ip, p.routed_subnets
                 FROM peers p JOIN users u ON u.id=p.user_id
                WHERE p.status NOT IN ('deleted','force_removed') AND u.status='active'"#,
    )
    .fetch_all(pool)
    .await
    .map_err(db)?;

    let mut leases = Vec::new();
    let mut site_sources = Vec::new();
    for (_, routed) in &peers {
        for cidr in routed.split(',').filter(|value| !value.is_empty()) {
            if let Ok(net) = cidr.parse::<Ipv4Net>() {
                if net.prefix_len() != 0 && net != vpn_subnet {
                    site_sources.push(net);
                }
            }
        }
    }
    let manual: Vec<(String, String)> = sqlx::query_as(
        r#"SELECT p.vpn_ip, g.routes FROM peers p
                 JOIN users u ON u.id=p.user_id
                 JOIN user_group_members m ON m.user_id=p.user_id
                 JOIN user_groups g ON g.id=m.group_id
                WHERE p.status NOT IN ('deleted','force_removed') AND u.status='active'"#,
    )
    .fetch_all(pool)
    .await
    .map_err(db)?;
    let grants: Vec<(String, String, i64)> = sqlx::query_as(
        r#"SELECT p.vpn_ip, g.routes, a.expires_at FROM peers p
                 JOIN users u ON u.id=p.user_id
                 JOIN access_grants a ON a.user_id=p.user_id
                 JOIN user_groups g ON g.id=a.group_id
                WHERE p.status NOT IN ('deleted','force_removed') AND u.status='active'
                  AND a.expires_at>?1"#,
    )
    .bind(now)
    .fetch_all(pool)
    .await
    .map_err(db)?;
    let legacy: Vec<(String,)> = sqlx::query_as(
        r#"SELECT p.vpn_ip FROM peers p JOIN users u ON u.id=p.user_id
                WHERE p.status NOT IN ('deleted','force_removed') AND u.status='active'
                  AND u.access_mode='legacy'
                  AND NOT EXISTS (SELECT 1 FROM user_group_members m WHERE m.user_id=u.id)
                  AND NOT EXISTS (SELECT 1 FROM access_grants a
                                   WHERE a.user_id=u.id AND a.expires_at>?1)"#,
    )
    .bind(now)
    .fetch_all(pool)
    .await
    .map_err(db)?;

    for (vpn_ip, routes) in manual {
        let source: Ipv4Addr = vpn_ip
            .parse()
            .map_err(|_| AppError::Validation("数据库中的 peer VPN IP 非法".into()))?;
        append_routes(&mut leases, source, &routes, ACL_LEASE_CAP_MS, vpn_subnet);
    }
    for (vpn_ip, routes, expires_at) in grants {
        let source: Ipv4Addr = vpn_ip
            .parse()
            .map_err(|_| AppError::Validation("数据库中的 peer VPN IP 非法".into()))?;
        let remaining_ms = expires_at.saturating_sub(now);
        let timeout = (remaining_ms as u64).clamp(1, ACL_LEASE_CAP_MS);
        append_routes(&mut leases, source, &routes, timeout, vpn_subnet);
    }
    // 历史账号维持“未分组回退全局路由”；受审批管控账号绝不回退。
    for (vpn_ip,) in legacy {
        let source: Ipv4Addr = vpn_ip
            .parse()
            .map_err(|_| AppError::Validation("数据库中的 peer VPN IP 非法".into()))?;
        for route in server_routes {
            append_routes(&mut leases, source, route, ACL_LEASE_CAP_MS, vpn_subnet);
        }
    }
    Ok((leases, site_sources))
}

fn append_routes(
    leases: &mut Vec<AclLease>,
    source: Ipv4Addr,
    csv: &str,
    timeout_ms: u64,
    vpn_subnet: Ipv4Net,
) {
    for route in csv
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let Ok(destination) = route.parse::<Ipv4Net>() else {
            tracing::warn!(route, "忽略数据库中的非法 ACL 路由");
            continue;
        };
        // 全隧道尚未获批；VPN 基础网段由固定规则处理，不作为普通授权元素。
        if destination.prefix_len() == 0 || destination == vpn_subnet {
            continue;
        }
        leases.push(AclLease {
            source,
            destination,
            timeout_ms,
        });
    }
}

fn db(error: sqlx::Error) -> AppError {
    AppError::Database(Box::new(error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
    use std::str::FromStr;

    async fn setup_pool() -> SqlitePool {
        let options = SqliteConnectOptions::from_str(
            "sqlite:file:network_acl_snapshot?mode=memory&cache=private",
        )
        .unwrap();
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        pool
    }

    #[test]
    fn route_snapshot_rejects_default_and_vpn_subnet() {
        let mut leases = Vec::new();
        append_routes(
            &mut leases,
            "10.8.0.2".parse().unwrap(),
            "0.0.0.0/0,10.8.0.0/24,192.168.1.0/24,bad",
            75,
            "10.8.0.0/24".parse().unwrap(),
        );
        assert_eq!(leases.len(), 1);
        assert_eq!(leases[0].destination, "192.168.1.0/24".parse().unwrap());
    }

    #[test]
    fn site_route_is_a_valid_bounded_acl_destination() {
        let mut leases = Vec::new();
        append_routes(
            &mut leases,
            "10.8.0.3".parse().unwrap(),
            "10.242.101.0/24",
            ACL_LEASE_CAP_MS,
            "10.8.0.0/24".parse().unwrap(),
        );

        assert_eq!(
            leases,
            vec![AclLease {
                source: "10.8.0.3".parse().unwrap(),
                destination: "10.242.101.0/24".parse().unwrap(),
                timeout_ms: ACL_LEASE_CAP_MS,
            }]
        );
    }

    #[tokio::test]
    async fn database_authorization_snapshot_only_leases_authorized_vpn_ips() {
        let pool = setup_pool().await;
        let now = 1_000_000_i64;
        for (id, ip, mode, routed_subnets) in [
            ("manual", "10.8.0.2", "approval_required", ""),
            ("approved", "10.8.0.3", "approval_required", ""),
            ("unapproved", "10.8.0.4", "approval_required", ""),
            ("legacy", "10.8.0.5", "legacy", ""),
            (
                "gateway",
                "10.8.0.6",
                "approval_required",
                "10.242.101.0/24",
            ),
        ] {
            sqlx::query(
                r#"INSERT INTO users
                   (id,username,email,password_hash,role,status,must_change_password,created_at,updated_at,access_mode)
                   VALUES (?1,?1,?2,'h','user','active',0,0,0,?3)"#,
            )
            .bind(id)
            .bind(format!("{id}@example.test"))
            .bind(mode)
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                r#"INSERT INTO peers
                   (id,user_id,device_name,wg_public_key,vpn_ip,status,created_at,updated_at,routed_subnets)
                   VALUES (?1,?1,'test',?2,?3,'online',0,0,?4)"#,
            )
            .bind(id)
            .bind(format!("pk-{id}"))
            .bind(ip)
            .bind(routed_subnets)
            .execute(&pool)
            .await
            .unwrap();
        }
        sqlx::query(
            r#"INSERT INTO user_groups (id,name,routes,created_at,updated_at)
               VALUES ('site-group','site','10.242.0.0/16',0,0)"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO user_group_members (user_id,group_id) VALUES ('manual','site-group')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            r#"INSERT INTO access_grants
               (id,approval_instance_code,user_id,group_id,expires_at,reason,created_at,updated_at)
               VALUES ('grant','approval','approved','site-group',?1,'',0,0)"#,
        )
        .bind(now + 60_000)
        .execute(&pool)
        .await
        .unwrap();

        let site: Ipv4Net = "10.242.101.0/24".parse().unwrap();
        let authorized_destination: Ipv4Net = "10.242.0.0/16".parse().unwrap();
        let (leases, site_sources) = build_acl_snapshot(
            &pool,
            &["172.31.9.0/24".to_string()],
            "10.8.0.0/24".parse().unwrap(),
            now,
        )
        .await
        .unwrap();

        assert!(site_sources.contains(&site));
        for source in ["10.8.0.2", "10.8.0.3"] {
            assert!(leases.iter().any(|lease| {
                lease.source == source.parse::<Ipv4Addr>().unwrap()
                    && lease.destination == authorized_destination
            }));
        }
        for source in ["10.8.0.4", "10.8.0.5", "10.8.0.6"] {
            assert!(!leases.iter().any(|lease| {
                lease.source == source.parse::<Ipv4Addr>().unwrap()
                    && lease.destination == authorized_destination
            }));
        }
        assert!(leases.iter().any(|lease| {
            lease.source == "10.8.0.5".parse::<Ipv4Addr>().unwrap()
                && lease.destination == "172.31.9.0/24".parse::<Ipv4Net>().unwrap()
        }));
    }
}
