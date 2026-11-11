//! Gateway selection and ownership changes shared by the CLI and desktop app.

use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    time::Duration,
};

use rand::RngCore;
use serde::{Deserialize, Serialize};
use tokio::{
    net::{lookup_host, UdpSocket},
    task::JoinSet,
    time::{timeout, Instant},
};
use vpn_api_types::peer::{PeerRegisterRequest, PeerRegisterResponse};

use crate::{
    api::ApiClient,
    error::{CliError, CliResult},
};

mod quality;

const PROBE_MAGIC: &[u8; 8] = b"YILIAN01";
const PROBE_ATTEMPTS: usize = 3;
const PROBE_TIMEOUT: Duration = Duration::from_millis(350);
const PROBE_DEADLINE: Duration = Duration::from_secs(2);
const SWITCH_DEADLINE: Duration = Duration::from_secs(22);
pub const INITIAL_HANDSHAKE_LIMIT: Duration = Duration::from_secs(20);
pub const RECOVERY_LIMIT: Duration = Duration::from_secs(12);
const FAILED_GATEWAY_COOLDOWN: Duration = Duration::from_secs(60);
const QUALITY_COOLDOWN: Duration = Duration::from_secs(60);
const QUALITY_REPORT_INTERVAL: Duration = Duration::from_secs(15);
const QUALITY_REQUEST_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_PROBED_GATEWAYS: usize = 64;

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Candidates {
    pub default_gateway_id: String,
    pub target_gateway_id: Option<String>,
    pub revision: i64,
    pub active_gateway_id: Option<String>,
    pub gateways: Vec<Gateway>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct Gateway {
    pub id: String,
    pub probe_endpoint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayChoice {
    pub id: String,
    pub revision: i64,
}

#[derive(Serialize)]
pub(crate) struct GatewayQuality {
    rtt_ms: Option<u64>,
    loss_pct: f64,
}

#[derive(Serialize)]
pub(crate) struct Selection<'a> {
    wg_public_key: &'a str,
    gateway_id: &'a str,
    revision: i64,
}

impl GatewayChoice {
    pub(crate) fn request<'a>(&'a self, wg_public_key: &'a str) -> Selection<'a> {
        Selection {
            wg_public_key,
            gateway_id: &self.id,
            revision: self.revision,
        }
    }
}

/// Local cooldown belongs to a single device connection, never to another login profile.
#[derive(Clone, Default)]
pub struct GatewayRetry {
    failures: HashMap<String, Instant>,
    preferred: Option<GatewayQualityMove>,
    quality_resume_at: Option<Instant>,
}

impl GatewayRetry {
    pub fn record_failure(&mut self, gateway: &GatewayChoice) {
        self.failures
            .insert(gateway.id.clone(), Instant::now() + FAILED_GATEWAY_COOLDOWN);
    }

    pub fn apply_reconnect(&mut self, reason: &GatewayReconnect) {
        match reason {
            GatewayReconnect::HandshakeFailed(gateway) => self.record_failure(gateway),
            GatewayReconnect::QualityImproved(movement) => {
                self.preferred = Some(movement.clone());
                self.quality_resume_at = Some(Instant::now() + QUALITY_COOLDOWN);
            }
            GatewayReconnect::SelectionChanged => {}
        }
    }

    /// The measured target only influences the immediate replacement attempt.
    pub fn for_connect_attempt(&mut self) -> Self {
        let preferred = self.preferred.take();
        Self {
            preferred,
            ..self.clone()
        }
    }

    fn quality_can_switch(&self) -> bool {
        self.quality_resume_at
            .is_none_or(|until| Instant::now() >= until)
    }

    fn cooling_down(&self, id: &str) -> bool {
        self.failures
            .get(id)
            .is_some_and(|until| *until > Instant::now())
    }
}

#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
struct Sample {
    successes: u8,
    mean: Duration,
}

async fn probe_endpoint(endpoint: String) -> Sample {
    timeout(PROBE_DEADLINE, async {
        let mut best = Sample::default();
        for addr in lookup_host(endpoint).await.ok()?.take(4) {
            let result = probe_address(addr, PROBE_ATTEMPTS, None)
                .await
                .unwrap_or_default();
            if result.successes > best.successes
                || (result.successes != 0
                    && result.successes == best.successes
                    && result.mean < best.mean)
            {
                best = result;
            }
            if best.successes == PROBE_ATTEMPTS as u8 {
                break;
            }
        }
        Some(best)
    })
    .await
    .ok()
    .flatten()
    .unwrap_or_default()
}

/// No sample if the socket could not send a probe outside the VPN.
async fn probe_address(
    addr: SocketAddr,
    attempts: usize,
    vpn_ip: Option<IpAddr>,
) -> Option<Sample> {
    let bind = if addr.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let Ok(socket) = UdpSocket::bind(bind).await else {
        return None;
    };
    if socket.connect(addr).await.is_err() {
        return None;
    }
    if vpn_ip.is_some() && socket.local_addr().ok().map(|addr| addr.ip()) == vpn_ip {
        return None;
    }
    let mut successes = 0;
    let mut elapsed = Duration::ZERO;
    let mut sent = false;
    for _ in 0..attempts {
        let mut request = [0; 24];
        request[..8].copy_from_slice(PROBE_MAGIC);
        rand::rngs::OsRng.fill_bytes(&mut request[8..]);
        let started = Instant::now();
        if socket.send(&request).await.is_err() {
            continue;
        }
        sent = true;
        let matched = timeout(PROBE_TIMEOUT, async {
            let mut response = [0; 25];
            while let Ok(len) = socket.recv(&mut response).await {
                if len == request.len() && response[..len] == request {
                    return true;
                }
            }
            false
        })
        .await
        .unwrap_or(false);
        if matched {
            successes += 1;
            elapsed += started.elapsed();
        }
    }
    sent.then(|| Sample {
        successes,
        mean: if successes == 0 {
            Duration::ZERO
        } else {
            elapsed / u32::from(successes)
        },
    })
}

impl Candidates {
    fn choice(&self, gateway: &Gateway) -> GatewayChoice {
        GatewayChoice {
            id: gateway.id.clone(),
            revision: self.revision,
        }
    }

    fn quality_preference<'a>(&self, retry: &'a GatewayRetry) -> Option<&'a str> {
        let GatewayQualityMove {
            current,
            next,
            paths,
        } = retry.preferred.as_ref()?;
        if self.target_gateway_id.is_some() {
            return None;
        }
        let available = |id: &str| {
            !retry.cooling_down(id) && self.gateways.iter().any(|gateway| gateway.id == id)
        };
        if self.revision == next.revision
            && available(&next.id)
            && quality::Paths::new(self, current).as_ref() == Some(paths)
        {
            return Some(&next.id);
        }
        self.keep_current_for_quality_retry(retry, current)
    }

    fn keep_current_for_quality_retry<'a>(
        &self,
        retry: &GatewayRetry,
        current: &'a GatewayChoice,
    ) -> Option<&'a str> {
        (self.active_gateway_id.as_deref() == Some(current.id.as_str())
            && !retry.cooling_down(&current.id)
            && self.gateways.iter().any(|gateway| gateway.id == current.id))
        .then_some(current.id.as_str())
    }

    pub(crate) async fn ranked(&self, retry: &GatewayRetry) -> Vec<GatewayChoice> {
        if self.gateways.len() <= 1 {
            return self
                .gateways
                .iter()
                .map(|gateway| self.choice(gateway))
                .collect();
        }
        let mut probes = JoinSet::new();
        let preferred = self.quality_preference(retry);
        let preferred_gateway =
            preferred.and_then(|id| self.gateways.iter().find(|node| node.id == id));
        for gateway in preferred_gateway
            .into_iter()
            .chain(
                self.gateways
                    .iter()
                    .filter(|node| Some(node.id.as_str()) != preferred),
            )
            .take(MAX_PROBED_GATEWAYS)
        {
            let gateway = gateway.clone();
            probes.spawn(async move {
                let result = probe_endpoint(gateway.probe_endpoint.clone()).await;
                (gateway, result)
            });
        }
        let mut ranked = Vec::new();
        while let Some(Ok((gateway, sample))) = probes.join_next().await {
            ranked.push((gateway, sample));
        }
        // A rolling winner must still answer at least one quick probe.
        // A dead target must not displace the still-active tunnel.
        let preferred = match retry.preferred.as_ref() {
            Some(GatewayQualityMove { current, next, .. })
                if preferred == Some(next.id.as_str())
                    && !ranked
                        .iter()
                        .any(|(node, sample)| node.id == next.id && sample.successes > 0) =>
            {
                self.keep_current_for_quality_retry(retry, current)
            }
            _ => preferred,
        };
        ranked.sort_by_key(|(gateway, sample)| {
            (
                retry.cooling_down(&gateway.id),
                self.target_gateway_id.as_deref() != Some(gateway.id.as_str()),
                preferred != Some(gateway.id.as_str()),
                std::cmp::Reverse(sample.successes),
                sample.mean,
                gateway.id != self.default_gateway_id,
                gateway.id.clone(),
            )
        });
        ranked
            .iter()
            .map(|(gateway, _)| self.choice(gateway))
            .collect()
    }

    /// An explicit target overrides probes while the control plane considers it ready.
    fn instructed_switch(&self, current: &GatewayChoice, retry: &GatewayRetry) -> bool {
        self.target_gateway_id.as_ref().is_some_and(|target| {
            target != &current.id
                && !retry.cooling_down(target)
                && self.gateways.iter().any(|gateway| &gateway.id == target)
        })
    }

    /// A withdrawn gateway must be replaced even if the old WireGuard session
    /// still appears healthy until its forwarding lease expires.
    fn removed_current(&self, current: &GatewayChoice) -> bool {
        self.active_gateway_id.as_deref() != Some(current.id.as_str())
            && !self.gateways.iter().any(|gateway| gateway.id == current.id)
    }
}

pub(crate) async fn enroll(
    api: &ApiClient,
    request: &PeerRegisterRequest,
) -> CliResult<PeerRegisterResponse> {
    timeout(SWITCH_DEADLINE, async {
        loop {
            match api.register_peer(request).await {
                Err(CliError::GatewayPending) => {
                    tokio::time::sleep(Duration::from_millis(750)).await
                }
                outcome => return outcome,
            }
        }
    })
    .await
    .unwrap_or(Err(CliError::GatewayPending))
}

/// Re-query on conflicts so a concurrent admin command never leaves us retrying
/// a superseded revision or a gateway that is no longer ready.
pub(crate) async fn acquire(
    api: &ApiClient,
    public_key: &str,
    mut candidates: Candidates,
    retry: &GatewayRetry,
) -> CliResult<(PeerRegisterResponse, GatewayChoice)> {
    let operation = async {
        'revision: for _ in 0..3 {
            let mut last_error = CliError::Other("当前没有可用的已授权 VPN 网关".into());
            for gateway in candidates.ranked(retry).await {
                loop {
                    match api.cluster_connect(public_key, &gateway).await {
                        Ok(config) => return Ok((config, gateway)),
                        Err(CliError::GatewayPending) => {
                            let latest = api
                                .cluster_candidates(public_key)
                                .await?
                                .ok_or(CliError::GatewayPending)?;
                            if latest.revision != gateway.revision
                                || latest.quality_preference(retry)
                                    != candidates.quality_preference(retry)
                            {
                                candidates = latest;
                                continue 'revision;
                            }
                            if !latest
                                .gateways
                                .iter()
                                .any(|candidate| candidate.id == gateway.id)
                            {
                                last_error = CliError::GatewayPending;
                                break;
                            }
                            tokio::time::sleep(Duration::from_millis(750)).await;
                        }
                        Err(
                            error @ CliError::Api {
                                code: vpn_api_types::error_codes::NO_ACCESS,
                                ..
                            },
                        ) => {
                            if let Some(latest) = api.cluster_candidates(public_key).await? {
                                if latest.revision != gateway.revision
                                    || latest.default_gateway_id != candidates.default_gateway_id
                                    || !latest
                                        .gateways
                                        .iter()
                                        .any(|candidate| candidate.id == gateway.id)
                                {
                                    candidates = latest;
                                    continue 'revision;
                                }
                            }
                            return Err(error);
                        }
                        Err(error) if error.stops_tunnel() => return Err(error),
                        Err(error) => {
                            last_error = error;
                            break;
                        }
                    }
                }
            }
            return Err(last_error);
        }
        Err(CliError::GatewayPending)
    };
    timeout(SWITCH_DEADLINE, operation)
        .await
        .unwrap_or(Err(CliError::GatewayPending))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayQualityMove {
    current: GatewayChoice,
    next: GatewayChoice,
    paths: quality::Paths,
}

/// Reason to rebuild a cluster tunnel while honoring the latest server policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatewayReconnect {
    SelectionChanged,
    QualityImproved(GatewayQualityMove),
    HandshakeFailed(GatewayChoice),
}

impl GatewayReconnect {
    pub fn failed_gateway(&self) -> Option<&GatewayChoice> {
        match self {
            Self::HandshakeFailed(gateway) => Some(gateway),
            Self::SelectionChanged | Self::QualityImproved(_) => None,
        }
    }
}

/// Watch the actual WireGuard health independently of control-plane heartbeats.
pub async fn monitor(
    api: std::sync::Arc<ApiClient>,
    public_key: String,
    mut gateway: GatewayChoice,
    shared: crate::daemon::SharedState,
    mut stop: tokio::sync::watch::Receiver<bool>,
    retry: GatewayRetry,
) -> Option<GatewayReconnect> {
    let mut ticks = tokio::time::interval(Duration::from_secs(1));
    let mut unhealthy_since = Instant::now();
    let mut heartbeats = shared.subscribe_heartbeats();
    let mut first_check = true;
    let mut retry_confirm_at = Instant::now();
    let mut network = crate::netmon::PollingMonitor::new(
        crate::netmon::OutboundIpProbe::default(),
        Duration::from_secs(5),
    );
    let mut last_network_check = Instant::now();
    let _ = network.poll_once().await;
    let mut confirmed_revision = None;
    let mut handshaken = false;
    let vpn_ip = shared
        .snapshot()
        .await
        .vpn_ip
        .and_then(|ip| ip.parse().ok());
    let mut quality = quality::History::default();
    let mut policy_updated = None;
    let mut quality_ticks = tokio::time::interval_at(
        Instant::now() + Duration::from_millis(rand::random::<u64>() % 2000),
        quality::INTERVAL,
    );
    quality_ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_sample_at: Option<Instant> = None;
    let mut next_report_at = Instant::now();
    loop {
        let (heartbeat, sample_due) = tokio::select! {
            biased;
            _ = stop.wait_for(|stopped| *stopped) => return None,
            _ = heartbeats.changed() => (true, false),
            _ = ticks.tick() => (false, false),
            _ = quality_ticks.tick(), if quality.paths().is_some() && vpn_ip.is_some() => (false, true),
        };
        let (data_healthy, control_healthy) = shared.channel_health().await;
        if data_healthy {
            handshaken = true;
            unhealthy_since = Instant::now();
        } else {
            let limit = if handshaken {
                RECOVERY_LIMIT
            } else {
                INITIAL_HANDSHAKE_LIMIT
            };
            if control_healthy && unhealthy_since.elapsed() >= limit {
                return Some(GatewayReconnect::HandshakeFailed(gateway));
            }
        }
        if !data_healthy {
            quality.clear();
        }
        if !control_healthy {
            quality.set_paths(None);
            policy_updated = None;
            first_check = true;
            continue;
        }
        let network_changed = if last_network_check.elapsed() >= network.interval() {
            last_network_check = Instant::now();
            network.poll_once().await.unwrap_or(false)
        } else {
            false
        };
        if network_changed {
            quality.clear();
        }
        let should_check = first_check || heartbeat || network_changed;
        first_check = false;
        let should_confirm = data_healthy
            && confirmed_revision != Some(gateway.revision)
            && retry_confirm_at <= Instant::now();
        if should_check || should_confirm {
            if should_confirm {
                retry_confirm_at = Instant::now() + Duration::from_secs(5);
            }
            let result = tokio::select! {
                biased;
                _ = stop.wait_for(|stopped| *stopped) => return None,
                result = async {
                    if should_check {
                        let latest = api.cluster_candidates(&public_key).await;
                        if !matches!(latest, Ok(Some(_))) {
                            quality.set_paths(None);
                            policy_updated = None;
                        }
                        if let Some(candidates) = latest? {
                            if candidates.removed_current(&gateway)
                                || candidates.instructed_switch(&gateway, &retry)
                            {
                                return Ok::<bool, CliError>(true);
                            }
                            gateway.revision = candidates.revision;
                            quality.set_paths(quality::Paths::new(&candidates, &gateway));
                            policy_updated = Some(Instant::now());
                        }
                    }
                    // Report completion only after an authenticated WireGuard handshake.
                    if data_healthy && (should_check || should_confirm) && confirmed_revision != Some(gateway.revision) {
                        api.cluster_confirm(&public_key, &gateway, None).await?;
                        confirmed_revision = Some(gateway.revision);
                    }
                    Ok(false)
                } => result,
            };
            match result {
                Ok(true) => return Some(GatewayReconnect::SelectionChanged),
                Ok(false) => {}
                Err(error) => {
                    tracing::warn!(error = %error.safe_diagnostic(), "VPN 网关状态暂不可更新");
                }
            }
        }
        let Some(vpn_ip) = vpn_ip.filter(|_| {
            data_healthy
                && policy_updated.is_some_and(|updated| updated.elapsed() < quality::POLICY_TTL)
        }) else {
            quality.clear();
            continue;
        };
        if !sample_due || confirmed_revision != Some(gateway.revision) {
            continue;
        }
        let now = Instant::now();
        if let Some(last) = last_sample_at {
            let gap = now.duration_since(last);
            // Missed ticks must not create a burst or bridge a sleep/network gap.
            if gap < quality::INTERVAL / 2 {
                continue;
            }
            if gap > 2 * quality::INTERVAL {
                quality.clear();
            }
        }
        last_sample_at = Some(now);
        if quality.paths().is_some() {
            let results = tokio::select! {
                biased;
                _ = stop.wait_for(|stopped| *stopped) => return None,
                results = quality.probe(vpn_ip) => results,
            };
            if let Some(next) = quality.observe(results, &gateway, &retry) {
                let latest = tokio::select! {
                    biased;
                    _ = stop.wait_for(|stopped| *stopped) => return None,
                    latest = timeout(QUALITY_REQUEST_TIMEOUT, api.cluster_candidates(&public_key)) => latest,
                };
                let Ok(Ok(Some(latest))) = latest else {
                    quality.set_paths(None);
                    policy_updated = None;
                    first_check = true;
                    continue;
                };
                if latest.removed_current(&gateway) || latest.instructed_switch(&gateway, &retry) {
                    return Some(GatewayReconnect::SelectionChanged);
                }
                let latest_paths = quality::Paths::new(&latest, &gateway);
                gateway.revision = latest.revision;
                policy_updated = Some(Instant::now());
                if quality.paths() != latest_paths.as_ref() {
                    quality.set_paths(latest_paths);
                    continue;
                }
                let Some(paths) = latest_paths else {
                    continue;
                };
                if shared.channel_health().await != (true, true) {
                    quality.clear();
                    continue;
                }
                tracing::info!(from = %gateway.id, to = %next.id, "备选 VPN 网关持续丢包更少，准备切换");
                return Some(GatewayReconnect::QualityImproved(GatewayQualityMove {
                    current: gateway,
                    next,
                    paths,
                }));
            }
            if now >= next_report_at {
                if let Some(measurement) = quality.measurement(&gateway) {
                    next_report_at = now + QUALITY_REPORT_INTERVAL;
                    let result = tokio::select! {
                        biased;
                        _ = stop.wait_for(|stopped| *stopped) => return None,
                        result = timeout(
                            QUALITY_REQUEST_TIMEOUT,
                            api.cluster_confirm(&public_key, &gateway, Some(&measurement)),
                        ) => result,
                    };
                    if let Ok(Err(error)) = result {
                        tracing::debug!(error = %error.safe_diagnostic(), "VPN 网关质量暂无法上报");
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn echo(mut reply: impl FnMut(&mut [u8]) -> bool + Send + 'static) -> String {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = socket.local_addr().unwrap();
        tokio::spawn(async move {
            let mut bytes = [0u8; 32];
            while let Ok((len, peer)) = socket.recv_from(&mut bytes).await {
                if reply(&mut bytes[..len]) {
                    socket.send_to(&bytes[..len], peer).await.unwrap();
                }
            }
        });
        addr.to_string()
    }

    #[tokio::test]
    async fn udp_probe_rejects_mismatched_nonce() {
        let endpoint = echo(|packet| {
            assert_eq!(packet.len(), 24);
            assert_eq!(&packet[..8], PROBE_MAGIC);
            packet[8] ^= 1;
            true
        })
        .await;
        assert_eq!(probe_endpoint(endpoint).await.successes, 0);
    }

    #[tokio::test]
    async fn an_explicit_ready_gateway_wins_but_a_failed_handshake_prefers_fallback() {
        let local = Gateway {
            id: "local".into(),
            probe_endpoint: echo(|_| true).await,
        };
        let remote = Gateway {
            id: "remote".into(),
            probe_endpoint: echo(|_| false).await,
        };
        let policy = Candidates {
            default_gateway_id: local.id.clone(),
            target_gateway_id: Some(remote.id.clone()),
            active_gateway_id: Some(local.id.clone()),
            revision: 42,
            gateways: vec![local, remote],
        };
        let mut retry = GatewayRetry::default();
        let choices = policy.ranked(&retry).await;
        assert!(policy.instructed_switch(&choices[1], &retry));
        assert_eq!(
            choices[0],
            GatewayChoice {
                id: "remote".into(),
                revision: 42
            }
        );
        retry.record_failure(&choices[0]);
        assert_eq!(policy.ranked(&retry).await[0].id, "local");
        assert!(!policy.instructed_switch(&choices[1], &retry));
    }

    #[tokio::test]
    async fn rolling_selection_survives_short_probes_only_under_current_auto_policy() {
        let local = Gateway {
            id: "local".into(),
            probe_endpoint: echo(|_| true).await,
        };
        let mut count = 0;
        let edge = Gateway {
            id: "edge".into(),
            probe_endpoint: echo(move |_| {
                count += 1;
                count == 1
            })
            .await,
        };
        let mut policy = Candidates {
            default_gateway_id: local.id.clone(),
            target_gateway_id: None,
            active_gateway_id: Some(local.id.clone()),
            revision: 42,
            gateways: vec![local, edge],
        };
        let mut retry = GatewayRetry::default();
        let next = policy.choice(&policy.gateways[1]);
        let current = policy.choice(&policy.gateways[0]);
        let paths = quality::Paths::new(&policy, &current).unwrap();
        retry.apply_reconnect(&GatewayReconnect::QualityImproved(GatewayQualityMove {
            current,
            next: next.clone(),
            paths,
        }));
        let attempt = retry.for_connect_attempt();
        assert_eq!(policy.ranked(&attempt).await[0], next);
        assert_eq!(policy.ranked(&attempt).await[0].id, "local");
        assert!(policy.quality_preference(&retry).is_none());
        policy.target_gateway_id = Some("local".into());
        assert!(policy.quality_preference(&attempt).is_none());
        policy.target_gateway_id = None;
        policy.revision += 1;
        assert_eq!(policy.quality_preference(&attempt), Some("local"));
        policy.revision -= 1;
        policy.gateways.retain(|node| node.id != next.id);
        assert_eq!(policy.quality_preference(&attempt), Some("local"));
    }

    #[tokio::test]
    async fn connected_probe_ignores_the_vpn_source_address() {
        let endpoint = echo(|_| true).await;
        let address: SocketAddr = endpoint.parse().unwrap();
        assert_eq!(probe_address(address, 1, Some(address.ip())).await, None);
        assert_eq!(
            probe_address(address, 1, Some("10.8.0.5".parse().unwrap()))
                .await
                .unwrap()
                .successes,
            1
        );
    }

    #[test]
    fn withdrawn_current_is_distinct_from_an_unready_but_still_assigned_gateway() {
        let current = GatewayChoice {
            id: "remote".into(),
            revision: 1,
        };
        let mut policy = Candidates {
            default_gateway_id: "local".into(),
            target_gateway_id: None,
            active_gateway_id: Some(current.id.clone()),
            revision: 2,
            gateways: vec![],
        };
        assert!(!policy.removed_current(&current));
        policy.active_gateway_id = None;
        assert!(policy.removed_current(&current));
    }
}
