//! Rolling loss on the physical paths to the currently eligible gateways.

use super::{
    probe_address, Candidates, Gateway, GatewayChoice, GatewayRetry, Sample, MAX_PROBED_GATEWAYS,
};
use std::{
    collections::VecDeque,
    net::{IpAddr, SocketAddr},
    time::Duration,
};
use tokio::{net::lookup_host, task::JoinSet, time::timeout};

pub(super) const INTERVAL: Duration = Duration::from_secs(2);
pub(super) const POLICY_TTL: Duration = Duration::from_secs(45);
const PROBE_DEADLINE: Duration = Duration::from_millis(1600);
const WINDOW: usize = 20;
const ASSESS_EVERY: usize = 5;
const MIN_GAIN: usize = 2;
const STREAK: u8 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Paths {
    revision: i64,
    default_id: String,
    gateways: Vec<Gateway>,
    can_switch: bool,
}

impl Paths {
    pub(super) fn new(policy: &Candidates, current: &GatewayChoice) -> Option<Self> {
        let current_node = policy.gateways.iter().find(|node| node.id == current.id)?;
        let can_switch = policy.target_gateway_id.is_none() && policy.gateways.len() > 1;
        let mut gateways = if can_switch {
            policy.gateways.clone()
        } else {
            vec![current_node.clone()]
        };
        gateways.sort_unstable_by(|a, b| a.id.cmp(&b.id));
        // Keep the active gateway in the sample even in a large cluster.
        if let Some(index) = gateways.iter().position(|node| node.id == current.id) {
            if index >= MAX_PROBED_GATEWAYS {
                gateways.swap(index, MAX_PROBED_GATEWAYS - 1);
            }
        }
        gateways.truncate(MAX_PROBED_GATEWAYS);
        Some(Self {
            revision: policy.revision,
            default_id: policy.default_gateway_id.clone(),
            gateways,
            can_switch,
        })
    }

    async fn probe(&self, vpn_ip: IpAddr, windows: &[Window]) -> Vec<Option<PathSample>> {
        let mut tasks = JoinSet::new();
        for (index, (gateway, window)) in self.gateways.iter().zip(windows).enumerate() {
            let endpoint = gateway.probe_endpoint.clone();
            let previous = window.address;
            tasks.spawn(async move {
                let result = timeout(PROBE_DEADLINE, async {
                    let mut addresses: Vec<_> = lookup_host(endpoint).await.ok()?.take(4).collect();
                    if let Some(address) = previous.filter(|address| addresses.contains(address)) {
                        return probe_address(address, 1, Some(vpn_ip))
                            .await
                            .map(|sample| PathSample { address, sample });
                    }
                    // Choose a working address when a hostname has multiple records.
                    // Once selected, keep the rolling window on that address.
                    addresses.sort_by_key(|addr| addr.is_ipv6());
                    let mut loss = None;
                    for address in addresses {
                        if let Some(sample) = probe_address(address, 1, Some(vpn_ip)).await {
                            let observed = PathSample { address, sample };
                            if sample.successes > 0 {
                                return Some(observed);
                            }
                            loss.get_or_insert(observed);
                        }
                    }
                    loss
                })
                .await
                .ok()
                .flatten();
                (index, result)
            });
        }
        let mut results = vec![None; self.gateways.len()];
        while let Some(Ok((index, result))) = tasks.join_next().await {
            results[index] = result;
        }
        results
    }
}

#[derive(Clone, Copy)]
pub(super) struct PathSample {
    address: SocketAddr,
    sample: Sample,
}

#[derive(Clone, Default)]
struct Window {
    address: Option<SocketAddr>,
    samples: VecDeque<Sample>,
}

#[derive(Default)]
pub(super) struct History {
    paths: Option<Paths>,
    windows: Vec<Window>,
    until_assessment: usize,
    leader: Option<(usize, u8)>,
}

impl History {
    pub(super) fn set_paths(&mut self, paths: Option<Paths>) {
        if self.paths == paths {
            return;
        }
        self.windows = paths.as_ref().map_or_else(Vec::new, |paths| {
            vec![Window::default(); paths.gateways.len()]
        });
        self.paths = paths;
        self.clear();
    }

    pub(super) fn clear(&mut self) {
        self.windows.fill_with(Window::default);
        self.until_assessment = ASSESS_EVERY;
        self.leader = None;
    }

    pub(super) fn paths(&self) -> Option<&Paths> {
        self.paths.as_ref()
    }

    pub(super) async fn probe(&self, vpn_ip: IpAddr) -> Vec<Option<PathSample>> {
        match &self.paths {
            Some(paths) => paths.probe(vpn_ip, &self.windows).await,
            None => Vec::new(),
        }
    }

    pub(super) fn measurement(&self, current: &GatewayChoice) -> Option<super::GatewayQuality> {
        let paths = self.paths.as_ref()?;
        if paths.revision != current.revision {
            return None;
        }
        let index = paths
            .gateways
            .iter()
            .position(|node| node.id == current.id)?;
        let window = &self.windows[index].samples;
        if window.len() != WINDOW {
            return None;
        }
        let successful = window.iter().filter(|sample| sample.successes > 0);
        let (count, total) = successful.fold((0u32, Duration::ZERO), |(count, total), sample| {
            (count + 1, total + sample.mean)
        });
        Some(super::GatewayQuality {
            rtt_ms: total.checked_div(count).map(|mean| mean.as_millis() as u64),
            loss_pct: (WINDOW - count as usize) as f64 * 100.0 / WINDOW as f64,
        })
    }

    pub(super) fn observe(
        &mut self,
        results: Vec<Option<PathSample>>,
        current: &GatewayChoice,
        retry: &GatewayRetry,
    ) -> Option<GatewayChoice> {
        let paths = self.paths.as_ref()?;
        for (index, (window, received)) in self.windows.iter_mut().zip(results).enumerate() {
            let address = received.map(|result| result.address);
            if window.address != address {
                window.samples.clear();
                window.address = address;
                if paths.gateways[index].id == current.id
                    || self.leader.is_some_and(|(leader, _)| leader == index)
                {
                    self.leader = None;
                }
            }
            if let Some(received) = received {
                if window.samples.len() == WINDOW {
                    window.samples.pop_front();
                }
                window.samples.push_back(received.sample);
            }
        }
        if !paths.can_switch {
            return None;
        }
        self.until_assessment -= 1;
        if self.until_assessment != 0 {
            return None;
        }
        self.until_assessment = ASSESS_EVERY;
        let success_count = |index: usize| {
            let samples = &self.windows[index].samples;
            (samples.len() == WINDOW)
                .then(|| samples.iter().filter(|sample| sample.successes > 0).count())
        };
        let current_index = paths
            .gateways
            .iter()
            .position(|node| node.id == current.id)?;
        let leader = success_count(current_index).and_then(|current_successes| {
            paths
                .gateways
                .iter()
                .enumerate()
                .filter(|(_, node)| !retry.cooling_down(&node.id))
                .filter_map(|(index, node)| {
                    let received = success_count(index)?;
                    (received >= current_successes + MIN_GAIN).then_some((
                        std::cmp::Reverse(received),
                        node.id != paths.default_id,
                        &node.id,
                        index,
                    ))
                })
                .min()
        });
        let Some((_, _, id, index)) = leader else {
            self.leader = None;
            return None;
        };
        let streak = self
            .leader
            .filter(|(previous, _)| *previous == index)
            .map_or(1, |(_, count)| count.saturating_add(1));
        self.leader = Some((index, streak));
        // Overlapping windows must not turn the same brief loss burst into
        // three independent indications that the path is still degraded.
        let recent_successes = |index: usize| {
            self.windows[index]
                .samples
                .iter()
                .rev()
                .take(ASSESS_EVERY * (usize::from(STREAK) - 1))
                .filter(|sample| sample.successes > 0)
                .count()
        };
        (streak >= STREAK
            && retry.quality_can_switch()
            && recent_successes(index) > recent_successes(current_index))
        .then(|| GatewayChoice {
            id: id.clone(),
            revision: paths.revision,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::GatewayReconnect;
    use tokio::time::Instant;

    fn sample(received: bool) -> Option<PathSample> {
        Some(PathSample {
            address: "127.0.0.1:1".parse().unwrap(),
            sample: Sample {
                successes: u8::from(received),
                mean: if received {
                    Duration::from_millis(25)
                } else {
                    Duration::ZERO
                },
            },
        })
    }

    fn policy() -> Candidates {
        Candidates {
            default_gateway_id: "local".into(),
            target_gateway_id: None,
            revision: 5,
            active_gateway_id: Some("local".into()),
            gateways: ["local", "edge-b", "edge-a"]
                .into_iter()
                .map(|id| Gateway {
                    id: id.into(),
                    probe_endpoint: "127.0.0.1:1".into(),
                })
                .collect(),
        }
    }

    fn setup() -> (History, GatewayChoice, GatewayRetry) {
        let policy = policy();
        let current = GatewayChoice {
            id: "local".into(),
            revision: policy.revision,
        };
        let mut history = History::default();
        history.set_paths(Paths::new(&policy, &current));
        (history, current, GatewayRetry::default())
    }

    #[test]
    fn only_switches_to_lowest_loss_after_three_full_assessments() {
        let (mut history, current, retry) = setup();
        // Sorted order: edge-a loses 5%, edge-b loses none, local loses 25%.
        for i in 1..=30 {
            let result = history.observe(
                vec![sample(i % 20 != 0), sample(true), sample(i % 4 != 0)],
                &current,
                &retry,
            );
            assert_eq!(
                result.map(|next| next.id),
                (i == 30).then(|| "edge-b".into())
            );
        }
    }

    #[test]
    fn single_loss_and_incomparable_paths_do_not_trigger_migration() {
        let (mut history, current, retry) = setup();
        for i in 0..60 {
            assert!(history
                .observe(vec![None, sample(true), sample(i != 17)], &current, &retry)
                .is_none());
        }
        history.clear();
        for i in 1..=60 {
            assert!(history
                .observe(
                    vec![None, sample(true), sample(i != 11 && i != 12)],
                    &current,
                    &retry
                )
                .is_none());
        }
        history.clear();
        for _ in 0..60 {
            assert!(history
                .observe(vec![None, sample(false), sample(false)], &current, &retry)
                .is_none());
        }
    }

    #[test]
    fn active_gateway_reports_rolling_rtt_and_loss_even_when_pinned() {
        let (mut history, current, retry) = setup();
        let mut pinned = policy();
        pinned.target_gateway_id = Some(current.id.clone());
        history.set_paths(Paths::new(&pinned, &current));
        for n in 0..40 {
            assert!(history
                .observe(vec![sample(n < 16)], &current, &retry)
                .is_none());
            if n < 19 {
                assert!(history.measurement(&current).is_none());
            }
            if n == 19 {
                let quality = history.measurement(&current).unwrap();
                assert_eq!(quality.rtt_ms, Some(25));
                assert_eq!(quality.loss_pct, 20.0);
            }
        }
        let quality = history.measurement(&current).unwrap();
        assert_eq!(quality.rtt_ms, None);
        assert_eq!(quality.loss_pct, 100.0);
        history.observe(vec![None], &current, &retry);
        assert!(history.measurement(&current).is_none());
    }

    #[test]
    fn candidate_changes_and_cooldown_prevent_reusing_a_previous_lead() {
        let (mut history, current, mut retry) = setup();
        let samples = vec![sample(true), sample(true), sample(false)];
        for _ in 0..29 {
            assert!(history.observe(samples.clone(), &current, &retry).is_none());
        }
        let mut changed = policy();
        changed.revision += 1;
        changed.gateways[1].probe_endpoint = "127.0.0.1:2".into();
        history.set_paths(Paths::new(&changed, &current));
        for _ in 0..29 {
            assert!(history.observe(samples.clone(), &current, &retry).is_none());
        }
        let next = history.observe(samples.clone(), &current, &retry).unwrap();
        assert_eq!(next.revision, changed.revision);
        retry.apply_reconnect(&GatewayReconnect::QualityImproved(
            super::super::GatewayQualityMove {
                current: current.clone(),
                next,
                paths: history.paths().unwrap().clone(),
            },
        ));
        for _ in 0..30 {
            assert!(history.observe(samples.clone(), &current, &retry).is_none());
        }
        retry.quality_resume_at = Some(Instant::now());
        assert!((0..5).any(|_| history.observe(samples.clone(), &current, &retry).is_some()));

        changed.target_gateway_id = Some("local".into());
        assert!(!Paths::new(&changed, &current).unwrap().can_switch);
        changed.target_gateway_id = None;
        changed.gateways.retain(|node| node.id == "local");
        assert!(!Paths::new(&changed, &current).unwrap().can_switch);
    }
}
