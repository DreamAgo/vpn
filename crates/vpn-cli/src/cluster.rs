//! Gateway selection and ownership changes shared by the CLI and desktop app.

use std::{collections::HashMap, net::SocketAddr, time::Duration};

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

const PROBE_MAGIC: &[u8; 8] = b"YILIAN01";
const PROBE_ATTEMPTS: usize = 3;
const PROBE_TIMEOUT: Duration = Duration::from_millis(350);
const PROBE_DEADLINE: Duration = Duration::from_secs(2);
const SWITCH_DEADLINE: Duration = Duration::from_secs(22);
pub const INITIAL_HANDSHAKE_LIMIT: Duration = Duration::from_secs(20);
pub const RECOVERY_LIMIT: Duration = Duration::from_secs(12);
const FAILED_GATEWAY_COOLDOWN: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Candidates {
    pub default_gateway_id: String,
    pub target_gateway_id: Option<String>,
    pub revision: i64,
    pub active_gateway_id: Option<String>,
    pub gateways: Vec<Gateway>,
}

#[derive(Debug, Clone, Deserialize)]
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
}

impl GatewayRetry {
    pub fn record_failure(&mut self, gateway: &GatewayChoice) {
        self.failures
            .insert(gateway.id.clone(), Instant::now() + FAILED_GATEWAY_COOLDOWN);
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
            let result = probe_address(addr).await;
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

async fn probe_address(addr: SocketAddr) -> Sample {
    let bind = if addr.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let Ok(socket) = UdpSocket::bind(bind).await else {
        return Sample::default();
    };
    if socket.connect(addr).await.is_err() {
        return Sample::default();
    }
    let mut successes = 0;
    let mut elapsed = Duration::ZERO;
    for _ in 0..PROBE_ATTEMPTS {
        let mut request = [0; 24];
        request[..8].copy_from_slice(PROBE_MAGIC);
        rand::rngs::OsRng.fill_bytes(&mut request[8..]);
        let started = Instant::now();
        if socket.send(&request).await.is_err() {
            continue;
        }
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
    Sample {
        successes,
        mean: if successes == 0 {
            Duration::ZERO
        } else {
            elapsed / u32::from(successes)
        },
    }
}

impl Candidates {
    fn choice(&self, gateway: &Gateway) -> GatewayChoice {
        GatewayChoice {
            id: gateway.id.clone(),
            revision: self.revision,
        }
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
        for gateway in self.gateways.iter().take(64) {
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
        ranked.sort_by_key(|(gateway, sample)| {
            (
                retry.cooling_down(&gateway.id),
                self.target_gateway_id.as_deref() != Some(gateway.id.as_str()),
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
                            if latest.revision != gateway.revision {
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

/// Reason to rebuild a cluster tunnel while honoring the latest server policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatewayReconnect {
    SelectionChanged,
    HandshakeFailed(GatewayChoice),
}

impl GatewayReconnect {
    pub fn failed_gateway(&self) -> Option<&GatewayChoice> {
        match self {
            Self::HandshakeFailed(gateway) => Some(gateway),
            Self::SelectionChanged => None,
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
    loop {
        let heartbeat = tokio::select! {
            biased;
            _ = stop.wait_for(|stopped| *stopped) => return None,
            _ = heartbeats.changed() => true,
            _ = ticks.tick() => false,
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
        if !control_healthy {
            continue;
        }
        let network_changed = if last_network_check.elapsed() >= network.interval() {
            last_network_check = Instant::now();
            network.poll_once().await.unwrap_or(false)
        } else {
            false
        };
        let should_check = first_check || heartbeat;
        first_check = false;
        let should_confirm = data_healthy
            && confirmed_revision != Some(gateway.revision)
            && retry_confirm_at <= Instant::now();
        if !should_check && !network_changed && !should_confirm {
            continue;
        }
        if should_confirm {
            retry_confirm_at = Instant::now() + Duration::from_secs(5);
        }
        let result = tokio::select! {
            biased;
            _ = stop.wait_for(|stopped| *stopped) => return None,
            result = async {
                if should_check || network_changed {
                    if let Some(candidates) = api.cluster_candidates(&public_key).await? {
                        if candidates.removed_current(&gateway)
                            || candidates.instructed_switch(&gateway, &retry)
                            || network_changed && candidates.ranked(&retry).await.first()
                                .is_some_and(|next| next.id != gateway.id)
                        {
                            return Ok::<bool, CliError>(true);
                        }
                        gateway.revision = candidates.revision;
                    }
                }
                // Only report completion after an authenticated WireGuard handshake.
                if data_healthy && (should_check || should_confirm) && confirmed_revision != Some(gateway.revision) {
                    api.cluster_confirm(&public_key, &gateway).await?;
                    confirmed_revision = Some(gateway.revision);
                }
                Ok(false)
            } => result,
        };
        match result {
            Ok(true) => return Some(GatewayReconnect::SelectionChanged),
            Ok(false) => {}
            Err(error) => {
                tracing::warn!(error = %error.safe_diagnostic(), "VPN 网关状态暂不可更新")
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
