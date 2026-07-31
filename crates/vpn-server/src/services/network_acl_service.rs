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
        let peers: Vec<(String, String)> = sqlx::query_as(
            r#"SELECT p.vpn_ip, p.routed_subnets
                 FROM peers p JOIN users u ON u.id=p.user_id
                WHERE p.status NOT IN ('deleted','force_removed') AND u.status='active'"#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;

        let mut leases = Vec::new();
        let mut site_sources = Vec::new();
        for (_, routed) in &peers {
            for cidr in routed.split(',').filter(|value| !value.is_empty()) {
                if let Ok(net) = cidr.parse::<Ipv4Net>() {
                    if net.prefix_len() != 0 && net != self.vpn_subnet {
                        site_sources.push(net);
                    }
                }
            }
        }
        // 启用审批时也审计迁移前的历史数据，不能只依赖新写入路径的校验。
        let groups: Vec<(String, String)> = sqlx::query_as("SELECT name, routes FROM user_groups")
            .fetch_all(&self.pool)
            .await
            .map_err(db)?;
        validate_group_site_separation(&groups, &site_sources)?;

        let manual: Vec<(String, String)> = sqlx::query_as(
            r#"SELECT p.vpn_ip, g.routes FROM peers p
                 JOIN users u ON u.id=p.user_id
                 JOIN user_group_members m ON m.user_id=p.user_id
                 JOIN user_groups g ON g.id=m.group_id
                WHERE p.status NOT IN ('deleted','force_removed') AND u.status='active'"#,
        )
        .fetch_all(&self.pool)
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
        .fetch_all(&self.pool)
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
        .fetch_all(&self.pool)
        .await
        .map_err(db)?;

        for (vpn_ip, routes) in manual {
            let source: Ipv4Addr = vpn_ip
                .parse()
                .map_err(|_| AppError::Validation("数据库中的 peer VPN IP 非法".into()))?;
            append_routes(
                &mut leases,
                source,
                &routes,
                ACL_LEASE_CAP_MS,
                self.vpn_subnet,
            );
        }
        for (vpn_ip, routes, expires_at) in grants {
            let source: Ipv4Addr = vpn_ip
                .parse()
                .map_err(|_| AppError::Validation("数据库中的 peer VPN IP 非法".into()))?;
            let remaining_ms = expires_at.saturating_sub(now);
            let timeout = (remaining_ms as u64).clamp(1, ACL_LEASE_CAP_MS);
            append_routes(&mut leases, source, &routes, timeout, self.vpn_subnet);
        }
        // 历史账号维持“未分组回退全局路由”；受审批管控账号绝不回退。
        for (vpn_ip,) in legacy {
            let source: Ipv4Addr = vpn_ip
                .parse()
                .map_err(|_| AppError::Validation("数据库中的 peer VPN IP 非法".into()))?;
            for route in &server_routes {
                append_routes(
                    &mut leases,
                    source,
                    route,
                    ACL_LEASE_CAP_MS,
                    self.vpn_subnet,
                );
            }
        }
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

fn overlaps(left: Ipv4Net, right: Ipv4Net) -> bool {
    left.contains(&right.network()) || right.contains(&left.network())
}

fn validate_group_site_separation(
    groups: &[(String, String)],
    site_sources: &[Ipv4Net],
) -> Result<()> {
    for (name, routes) in groups {
        for route in routes
            .split(',')
            .filter_map(|route| route.parse::<Ipv4Net>().ok())
        {
            if site_sources.iter().any(|site| overlaps(route, *site)) {
                return Err(AppError::Validation(format!(
                    "用户组 {name} 的授权网段 {route} 与站点源网段重叠"
                )));
            }
        }
    }
    Ok(())
}

fn db(error: sqlx::Error) -> AppError {
    AppError::Database(Box::new(error))
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn startup_audit_rejects_historical_group_site_overlap() {
        let groups = vec![("production".into(), "192.168.20.0/24".into())];
        assert!(
            validate_group_site_separation(&groups, &["192.168.20.128/25".parse().unwrap()])
                .is_err()
        );
    }
}
