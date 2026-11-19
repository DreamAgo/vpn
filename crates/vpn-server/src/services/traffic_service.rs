//! Durable counter deltas and hourly traffic history, from the client's perspective.
use std::{collections::HashSet, sync::Arc, time::Duration};

use sqlx::SqlitePool;
use tokio::sync::Mutex;
use vpn_api_types::traffic::{
    PeerTraffic, TrafficBytes, TrafficDashboard, TrafficPoint, TrafficReport, TrafficSource,
};
use vpn_core::{AppError, Result};

use super::PeerService;

pub const SAMPLE_INTERVAL_MS: i64 = 5_000;
const FRESH_MS: i64 = 20_000;
const HOUR_MS: i64 = 3_600_000;
const DAY_MS: i64 = 24 * HOUR_MS;

pub struct TrafficService {
    pool: SqlitePool,
    // Serializes local samples and authenticated remote reports, including concurrent API reads.
    gate: Mutex<()>,
}

#[derive(sqlx::FromRow)]
struct PeerTrafficRow {
    peer_id: String,
    device_name: String,
    username: String,
    vpn_ip: String,
    upload_bytes: i64,
    download_bytes: i64,
    upload_bps: Option<f64>,
    download_bps: Option<f64>,
    sampled_at: i64,
}

fn db(error: sqlx::Error) -> AppError {
    AppError::Database(Box::new(error))
}

impl TrafficService {
    pub fn new(pool: SqlitePool) -> Self {
        Self {
            pool,
            gate: Mutex::new(()),
        }
    }

    pub fn spawn(self: &Arc<Self>, peers: Arc<PeerService>) {
        let service = self.clone();
        tokio::spawn(async move {
            let generation = uuid::Uuid::new_v4().to_string();
            let mut sequence = 0;
            let mut ticker =
                tokio::time::interval(Duration::from_millis(SAMPLE_INTERVAL_MS as u64));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                sequence += 1;
                let result = async {
                    if let Some(counters) = peers.traffic_counters().await? {
                        service
                            .record(
                                "local",
                                &TrafficReport {
                                    generation: generation.clone(),
                                    sequence,
                                    counters,
                                },
                                chrono::Utc::now().timestamp_millis(),
                            )
                            .await?;
                    }
                    Ok::<_, AppError>(())
                }
                .await;
                if let Err(error) = result {
                    tracing::warn!(%error, "本地流量采集失败");
                }
            }
        });
    }

    pub async fn record(&self, source: &str, report: &TrafficReport, now: i64) -> Result<()> {
        let mut keys = HashSet::new();
        if report.generation.len() > 64
            || uuid::Uuid::parse_str(&report.generation).is_err()
            || report.sequence < 0
            || report.counters.len() > 65_536
            || report.counters.iter().any(|c| {
                c.rx_bytes > i64::MAX as u64
                    || c.tx_bytes > i64::MAX as u64
                    || c.public_key.len() > 64
                    || !keys.insert(&c.public_key)
            })
        {
            return Err(AppError::Validation("流量报告无效".into()));
        }
        let _guard = self.gate.lock().await;
        let mut tx = self.pool.begin().await.map_err(db)?;
        // A write before reading also protects against another service process.
        sqlx::query("INSERT OR IGNORE INTO traffic_sources(id,generation,sequence,sampled_at,first_sample_at) VALUES(?,?,-1,?,?)")
            .bind(source).bind(&report.generation).bind(now).bind(now)
            .execute(&mut *tx).await.map_err(db)?;
        let (generation, sequence, previous_at): (String, i64, i64) =
            sqlx::query_as("SELECT generation,sequence,sampled_at FROM traffic_sources WHERE id=?")
                .bind(source)
                .fetch_one(&mut *tx)
                .await
                .map_err(db)?;
        if (generation == report.generation && sequence >= report.sequence)
            || (sequence >= 0 && now < previous_at)
        {
            return Ok(());
        }
        // Data is a complete interface snapshot. Missing peers are rebased if they reappear.
        let previous: Vec<(String, String, String, i64, i64, bool)> =
            sqlx::query_as("SELECT public_key,peer_id,generation,rx,tx,active FROM traffic_counters WHERE source_id=?")
                .bind(source).fetch_all(&mut *tx).await.map_err(db)?;
        let previous: std::collections::HashMap<_, _> = previous
            .into_iter()
            .map(|(key, peer, generation, rx, tx, active)| {
                (key, (peer, generation, rx, tx, active))
            })
            .collect();
        sqlx::query("UPDATE traffic_counters SET active=0 WHERE source_id=?")
            .bind(source)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        for counter in &report.counters {
            let peer: Option<(String, String, String, String)> = if source == "local" {
                sqlx::query_as("SELECT p.id,p.device_name,u.username,p.vpn_ip FROM peers p JOIN users u ON u.id=p.user_id WHERE p.wg_public_key=?")
                    .bind(&counter.public_key).fetch_optional(&mut *tx).await.map_err(db)?
            } else {
                // A gateway can account only for peers actually assigned to it.
                sqlx::query_as("SELECT p.id,p.device_name,u.username,p.vpn_ip FROM peers p JOIN users u ON u.id=p.user_id JOIN cluster_remote_leases l ON l.peer_id=p.id AND l.public_key=p.wg_public_key WHERE p.wg_public_key=? AND l.gateway_id=?")
                    .bind(&counter.public_key).bind(source).fetch_optional(&mut *tx).await.map_err(db)?
            };
            let Some((id, name, username, ip)) = peer else {
                continue;
            };
            let rx = counter.rx_bytes as i64;
            let tx_bytes = counter.tx_bytes as i64;
            let (upload, download, has_baseline) = match previous.get(&counter.public_key) {
                Some((peer, epoch, prev_rx, prev_tx, true))
                    if peer == &id && epoch == &report.generation =>
                {
                    // A peer can be removed/recreated independently of the gateway process.
                    (
                        if rx >= *prev_rx { rx - prev_rx } else { rx },
                        if tx_bytes >= *prev_tx {
                            tx_bytes - prev_tx
                        } else {
                            tx_bytes
                        },
                        true,
                    )
                }
                _ => (0, 0, false),
            };
            let elapsed = now - previous_at;
            let rate = |bytes: i64| {
                (has_baseline && elapsed > 0 && elapsed <= FRESH_MS)
                    .then(|| bytes as f64 * 1000.0 / elapsed as f64)
            };
            sqlx::query("INSERT INTO traffic_counters(source_id,public_key,peer_id,generation,rx,tx,active,upload_bps,download_bps,sampled_at) VALUES(?,?,?,?,?,?,1,?,?,?) ON CONFLICT(source_id,public_key) DO UPDATE SET peer_id=excluded.peer_id,generation=excluded.generation,rx=excluded.rx,tx=excluded.tx,active=1,upload_bps=excluded.upload_bps,download_bps=excluded.download_bps,sampled_at=excluded.sampled_at")
                .bind(source).bind(&counter.public_key).bind(&id).bind(&report.generation).bind(rx).bind(tx_bytes)
                .bind(rate(upload)).bind(rate(download)).bind(now).execute(&mut *tx).await.map_err(db)?;
            sqlx::query("INSERT INTO traffic_totals(peer_id,device_name,username,vpn_ip,upload_bytes,download_bytes,sampled_at) VALUES(?,?,?,?,?,?,?) ON CONFLICT(peer_id) DO UPDATE SET device_name=excluded.device_name,username=excluded.username,vpn_ip=excluded.vpn_ip,upload_bytes=upload_bytes+excluded.upload_bytes,download_bytes=download_bytes+excluded.download_bytes,sampled_at=excluded.sampled_at")
                .bind(&id).bind(name).bind(username).bind(ip).bind(upload).bind(download)
                .bind(now).execute(&mut *tx).await.map_err(db)?;
            if upload != 0 || download != 0 {
                sqlx::query("INSERT INTO traffic_hourly(hour,peer_id,upload_bytes,download_bytes) VALUES(?,?,?,?) ON CONFLICT(hour,peer_id) DO UPDATE SET upload_bytes=upload_bytes+excluded.upload_bytes,download_bytes=download_bytes+excluded.download_bytes")
                    .bind(now.div_euclid(HOUR_MS)*HOUR_MS).bind(&id).bind(upload).bind(download)
                    .execute(&mut *tx).await.map_err(db)?;
            }
        }
        sqlx::query("UPDATE traffic_sources SET generation=?,sequence=?,sampled_at=? WHERE id=?")
            .bind(&report.generation)
            .bind(report.sequence)
            .bind(now)
            .bind(source)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        sqlx::query("DELETE FROM traffic_hourly WHERE hour<?")
            .bind(now - 90 * DAY_MS)
            .execute(&mut *tx)
            .await
            .map_err(db)?;
        tx.commit().await.map_err(db)
    }

    pub async fn dashboard(
        &self,
        days: u32,
        sources: Vec<(String, String)>,
        now: i64,
    ) -> Result<TrafficDashboard> {
        if ![1, 7, 30].contains(&days) {
            return Err(AppError::Validation("统计范围须为 1、7 或 30 天".into()));
        }
        let mut tx = self.pool.begin().await.map_err(db)?;
        // Buckets align to Shanghai local days; a day means the current day plus prior days.
        let today = (now + 8 * HOUR_MS).div_euclid(DAY_MS) * DAY_MS - 8 * HOUR_MS;
        let from = if days == 1 {
            now.div_euclid(HOUR_MS) * HOUR_MS - 23 * HOUR_MS
        } else {
            today - (i64::from(days) - 1) * DAY_MS
        };
        let bucket = if days == 1 { HOUR_MS } else { DAY_MS };
        let offset = if days == 1 { 0 } else { 8 * HOUR_MS };
        let rows: Vec<(i64, i64, i64)> = sqlx::query_as("SELECT ((hour+?)/?)*?-?,SUM(upload_bytes),SUM(download_bytes) FROM traffic_hourly WHERE hour>=? GROUP BY 1 ORDER BY 1")
            .bind(offset).bind(bucket).bind(bucket).bind(offset).bind(from)
            .fetch_all(&mut *tx).await.map_err(db)?;
        let points: std::collections::BTreeMap<_, _> =
            rows.into_iter().map(|(t, u, d)| (t, (u, d))).collect();
        let series: Vec<_> = (from..=now)
            .step_by(bucket as usize)
            .map(|timestamp| {
                let (upload_bytes, download_bytes) =
                    points.get(&timestamp).copied().unwrap_or_default();
                TrafficPoint {
                    timestamp,
                    upload_bytes,
                    download_bytes,
                }
            })
            .collect();
        let period = TrafficBytes {
            upload_bytes: series.iter().map(|p| p.upload_bytes).sum(),
            download_bytes: series.iter().map(|p| p.download_bytes).sum(),
        };
        let (up,down): (i64,i64) = sqlx::query_as("SELECT COALESCE(SUM(upload_bytes),0),COALESCE(SUM(download_bytes),0) FROM traffic_totals")
            .fetch_one(&mut *tx).await.map_err(db)?;
        let total = TrafficBytes {
            upload_bytes: up,
            download_bytes: down,
        };
        let (up,down): (i64,i64) = sqlx::query_as("SELECT COALESCE(SUM(upload_bytes),0),COALESCE(SUM(download_bytes),0) FROM traffic_hourly WHERE hour>=?")
            .bind(today).fetch_one(&mut *tx).await.map_err(db)?;
        let today = TrafficBytes {
            upload_bytes: up,
            download_bytes: down,
        };
        let rows: Vec<PeerTrafficRow> = sqlx::query_as(
            "SELECT t.peer_id,t.device_name,t.username,t.vpn_ip,COALESCE(h.u,0) AS upload_bytes,COALESCE(h.d,0) AS download_bytes,r.u AS upload_bps,r.d AS download_bps,t.sampled_at FROM traffic_totals t LEFT JOIN (SELECT peer_id,SUM(upload_bps) u,SUM(download_bps) d FROM traffic_counters WHERE active=1 AND sampled_at>=? AND sampled_at<=? GROUP BY peer_id) r ON r.peer_id=t.peer_id LEFT JOIN (SELECT peer_id,SUM(upload_bytes) u,SUM(download_bytes) d FROM traffic_hourly WHERE hour>=? GROUP BY peer_id) h ON h.peer_id=t.peer_id ORDER BY COALESCE(h.u,0)+COALESCE(h.d,0) DESC,t.peer_id")
            .bind(now-FRESH_MS).bind(now).bind(from).fetch_all(&mut *tx).await.map_err(db)?;
        let peers: Vec<_> = rows
            .into_iter()
            .map(|row| PeerTraffic {
                peer_id: row.peer_id,
                device_name: row.device_name,
                username: row.username,
                vpn_ip: row.vpn_ip,
                upload_bytes: row.upload_bytes,
                download_bytes: row.download_bytes,
                upload_bps: row.upload_bps,
                download_bps: row.download_bps,
                sampled_at: row.sampled_at,
            })
            .collect();
        let fresh_count = peers.iter().filter(|p| p.upload_bps.is_some()).count();
        let upload_bps = (fresh_count > 0).then(|| peers.iter().filter_map(|p| p.upload_bps).sum());
        let download_bps =
            (fresh_count > 0).then(|| peers.iter().filter_map(|p| p.download_bps).sum());
        let stamps: Vec<(String, i64)> =
            sqlx::query_as("SELECT id,sampled_at FROM traffic_sources")
                .fetch_all(&mut *tx)
                .await
                .map_err(db)?;
        let sources = sources
            .into_iter()
            .map(|(id, name)| {
                let sampled_at = stamps
                    .iter()
                    .find(|(key, _)| key == &id)
                    .map(|(_, time)| *time);
                TrafficSource {
                    id,
                    name,
                    sampled_at,
                }
            })
            .collect();
        let first_sample_at: Option<i64> =
            sqlx::query_scalar("SELECT MIN(first_sample_at) FROM traffic_sources")
                .fetch_one(&mut *tx)
                .await
                .map_err(db)?;
        tx.commit().await.map_err(db)?;
        Ok(TrafficDashboard {
            from,
            to: now,
            first_sample_at,
            sources,
            period,
            today,
            total,
            upload_bps,
            download_bps,
            series,
            peers,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vpn_api_types::traffic::TrafficCounter;

    async fn fixture() -> (SqlitePool, TrafficService) {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        sqlx::query("INSERT INTO users(id,username,email,password_hash,role,created_at,updated_at) VALUES('u','alice','a@example.test','hash','user',0,0)")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO peers(id,user_id,device_name,wg_public_key,vpn_ip,status,created_at,updated_at) VALUES('p','u','Laptop','key','10.8.0.2','online',0,0)")
            .execute(&pool).await.unwrap();
        (pool.clone(), TrafficService::new(pool))
    }

    fn report(sequence: i64, rx: u64, tx: u64) -> TrafficReport {
        TrafficReport {
            generation: "019d5555-5555-7555-8555-555555555555".into(),
            sequence,
            counters: vec![TrafficCounter {
                public_key: "key".into(),
                rx_bytes: rx,
                tx_bytes: tx,
            }],
        }
    }

    async fn view(service: &TrafficService, now: i64) -> TrafficDashboard {
        service
            .dashboard(1, vec![("local".into(), "内置网关".into())], now)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn persists_deltas_handles_resets_and_fences_duplicate_samples() {
        let (pool, service) = fixture().await;
        let now = 1_800_000_000_000;
        service
            .record("local", &report(1, 1000, 2000), now)
            .await
            .unwrap();
        assert_eq!(view(&service, now).await.total.upload_bytes, 0);
        service
            .record("local", &report(2, 1500, 3000), now + 5000)
            .await
            .unwrap();
        service
            .record("local", &report(2, 1500, 3000), now + 6000)
            .await
            .unwrap();
        service
            .record("local", &report(1, 1, 1), now + 7000)
            .await
            .unwrap();
        let restarted = TrafficService::new(pool);
        let stats = view(&restarted, now + 7000).await;
        assert_eq!(stats.total.upload_bytes, 500);
        assert_eq!(stats.total.download_bytes, 1000);
        assert_eq!(stats.upload_bps, Some(100.0));
        assert_eq!(stats.download_bps, Some(200.0));
        assert_eq!(stats.period.upload_bytes, 500);
        assert_eq!(stats.peers[0].username, "alice");
        assert_eq!(stats.series.len(), 24);
        restarted
            .record("local", &report(3, 100, 200), now + 10000)
            .await
            .unwrap();
        assert_eq!(view(&restarted, now + 10000).await.total.upload_bytes, 600);
        // No network data is generated by a new process's initial baseline.
        let mut reboot = report(1, 50000, 50000);
        reboot.generation = uuid::Uuid::new_v4().to_string();
        restarted
            .record("local", &reboot, now + 15000)
            .await
            .unwrap();
        restarted
            .record("local", &report(4, 90000, 90000), now + 14000)
            .await
            .unwrap();
        let stats = view(&restarted, now + 15000).await;
        assert_eq!(stats.total.upload_bytes, 600);
        assert!(stats.upload_bps.is_none());
        assert!(view(&restarted, now + 40000).await.peers[0]
            .download_bps
            .is_none());
    }

    #[tokio::test]
    async fn missing_peers_rebase_and_remote_sources_cannot_claim_unassigned_peers() {
        let (pool, service) = fixture().await;
        let now = 1_800_000_000_000;
        service
            .record("edge", &report(1, 100, 200), now)
            .await
            .unwrap();
        assert!(view(&service, now).await.peers.is_empty());
        sqlx::query("INSERT INTO cluster_remote_leases(vpn_ip,peer_id,public_key,gateway_id,client_until) VALUES('10.8.0.2','p','key','edge',?)")
            .bind(now+100000).execute(&pool).await.unwrap();
        service
            .record("edge", &report(2, 100, 200), now + 5000)
            .await
            .unwrap();
        service
            .record("edge", &report(3, 300, 700), now + 10000)
            .await
            .unwrap();
        assert_eq!(view(&service, now + 10000).await.total.download_bytes, 500);
        let mut empty = report(4, 0, 0);
        empty.counters.clear();
        service.record("edge", &empty, now + 15000).await.unwrap();
        assert!(view(&service, now + 15000).await.upload_bps.is_none());
        service
            .record("edge", &report(5, 10000, 20000), now + 20000)
            .await
            .unwrap();
        assert_eq!(view(&service, now + 20000).await.total.upload_bytes, 200);
        let mut duplicate = report(6, 20000, 30000);
        duplicate.counters.push(duplicate.counters[0].clone());
        assert!(service
            .record("edge", &duplicate, now + 25000)
            .await
            .is_err());
        assert_eq!(view(&service, now + 25000).await.total.download_bytes, 500);
    }

    #[tokio::test]
    async fn shanghai_midnight_separates_today_from_yesterday_and_retains_lifetime_totals() {
        let (_, service) = fixture().await;
        let midnight = chrono::DateTime::parse_from_rfc3339("2026-11-19T00:00:00+08:00")
            .unwrap()
            .timestamp_millis();
        service
            .record("local", &report(1, 0, 0), midnight - 10000)
            .await
            .unwrap();
        service
            .record("local", &report(2, 100, 200), midnight - 5000)
            .await
            .unwrap();
        service
            .record("local", &report(3, 400, 600), midnight + 5000)
            .await
            .unwrap();
        let stats = service
            .dashboard(7, vec![], midnight + 10000)
            .await
            .unwrap();
        assert_eq!(stats.today.upload_bytes, 300);
        assert_eq!(stats.total.upload_bytes, 400);
        assert_eq!(stats.series.len(), 7);
        assert_eq!(stats.series[5].upload_bytes, 100);
        assert_eq!(stats.series[6].upload_bytes, 300);
        assert!(service.dashboard(365, vec![], midnight).await.is_err());
    }
}
