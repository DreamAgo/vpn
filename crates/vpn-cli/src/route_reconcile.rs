//! Route ownership and reconciliation. Never adopt an existing system route.
//!
//! Keep failed deletions in the ledger so reconnect/periodic reconciliation can
//! retry them without installing a conflicting replacement. All tests use an
//! in-memory backend; no test changes the host routing table.

use std::{io, net::IpAddr};

use async_trait::async_trait;
use net_route::{Handle, Route};

#[async_trait]
trait RouteBackend: Send + Sync {
    async fn list(&self) -> io::Result<Vec<Route>>;
    async fn add(&self, route: &Route) -> io::Result<()>;
    async fn delete(&self, route: &Route) -> io::Result<()>;

    /// net-route 0.4.6 deletes by destination/prefix on macOS and by
    /// destination/prefix/metric on Linux, ignoring interface/gateway/table.
    /// Require a unique prefix on both platforms before calling that API.
    fn deletion_requires_unique_prefix(&self) -> bool;
}

#[async_trait]
impl RouteBackend for Handle {
    async fn list(&self) -> io::Result<Vec<Route>> {
        Handle::list(self).await
    }

    async fn add(&self, route: &Route) -> io::Result<()> {
        Handle::add(self, route).await
    }

    async fn delete(&self, route: &Route) -> io::Result<()> {
        Handle::delete(self, route).await
    }

    fn deletion_requires_unique_prefix(&self) -> bool {
        !cfg!(target_os = "windows")
    }
}

fn same_prefix(left: &Route, right: &Route) -> bool {
    left.destination == right.destination && left.prefix == right.prefix
}

fn gateway(route: &Route) -> Option<IpAddr> {
    // Windows reads on-link next hops back as Some(0.0.0.0) / Some(::),
    // although the route was added with None.
    route.gateway.filter(|address| !address.is_unspecified())
}

fn same_route(left: &Route, right: &Route) -> bool {
    if !same_prefix(left, right) || left.ifindex != right.ifindex || gateway(left) != gateway(right)
    {
        return false;
    }
    #[cfg(target_os = "linux")]
    if left.table != right.table
        || left.source != right.source
        || left.source_prefix != right.source_prefix
    {
        return false;
    }
    // Do not compare Route::eq: table reads may populate metric, Windows
    // LUID, and Linux source_hint even when the add request omitted them.
    true
}

pub(crate) async fn reconcile(
    handle: &Handle,
    added: &mut Vec<(String, Route)>,
    desired: &[Route],
) {
    reconcile_with(handle, added, desired).await;
}

/// Return the number of owned routes still requiring cleanup.
pub(crate) async fn cleanup(handle: &Handle, added: &mut Vec<(String, Route)>) -> usize {
    reconcile_with(handle, added, &[]).await;
    added.len()
}

async fn list_routes(backend: &impl RouteBackend) -> Option<Vec<Route>> {
    match backend.list().await {
        Ok(routes) => Some(routes),
        Err(error) => {
            tracing::warn!(
                error = %crate::error::redact_sensitive(&error.to_string()),
                "读取路由表失败，保留路由和所有权记录，等待重试"
            );
            None
        }
    }
}

async fn reconcile_with(
    backend: &impl RouteBackend,
    added: &mut Vec<(String, Route)>,
    desired: &[Route],
) {
    // A failed snapshot must not turn into an empty routing table and cause
    // route deletions, ownership loss, or an unsafe add.
    let Some(mut current) = list_routes(backend).await else {
        return;
    };
    let mut removed_for_replacement: Vec<(String, Route)> = Vec::new();

    let mut index = 0;
    while index < added.len() {
        let (label, owned) = &added[index];
        if !current.iter().any(|route| same_route(route, owned)) {
            // An administrator or a network change already removed/replaced
            // our route. Release only the record; leave every system route.
            added.remove(index);
            continue;
        }
        if desired.iter().any(|route| same_route(route, owned)) {
            index += 1;
            continue;
        }

        // Re-read immediately before deleting. In particular, the macOS API
        // cannot target an interface, so a stale record must never authorize
        // deleting a third-party replacement of the same prefix.
        let Some(fresh) = list_routes(backend).await else {
            return;
        };
        current = fresh;
        let Some(observed) = current.iter().find(|route| same_route(route, owned)) else {
            added.remove(index);
            continue;
        };
        if backend.deletion_requires_unique_prefix()
            && current
                .iter()
                .filter(|route| same_prefix(route, owned))
                .count()
                != 1
        {
            tracing::warn!(route = %label, "同网段存在其他路由，保留本次路由记录并跳过不安全的删除");
            index += 1;
            continue;
        }

        // Use the observed row: Linux's delete compares the metric exactly,
        // and Windows benefits from the resolved interface LUID/next hop.
        match backend.delete(observed).await {
            Ok(()) => {
                current.retain(|route| !same_route(route, owned));
                tracing::debug!(route = %label, "已删除本次连接添加的路由");
                let needs_replacement = desired.iter().any(|route| same_prefix(route, owned));
                let removed = added.remove(index);
                if needs_replacement {
                    removed_for_replacement.push(removed);
                }
            }
            Err(error) => {
                tracing::warn!(
                    route = %label,
                    error = %crate::error::redact_sensitive(&error.to_string()),
                    "删除路由失败，确认路由状态后保留记录以便重试"
                );
                let Some(fresh) = list_routes(backend).await else {
                    return;
                };
                current = fresh;
                if current.iter().any(|route| same_route(route, owned)) {
                    index += 1;
                } else {
                    // Even NotFound errors require a successful table read:
                    // a backend's incomplete delete key can falsely miss it.
                    added.remove(index);
                }
            }
        }
    }

    if desired.is_empty() {
        return;
    }
    // Refresh after all removals and before attempting additions, including
    // when another process supplied a route while removal was in progress.
    let Some(fresh) = list_routes(backend).await else {
        return;
    };
    current = fresh;
    for desired_route in desired {
        if added
            .iter()
            .any(|(_, route)| same_prefix(route, desired_route))
        {
            // A failed removal blocks replacement even if the new interface
            // could otherwise coexist with the stale route on this OS.
            continue;
        }
        if current
            .iter()
            .any(|route| same_prefix(route, desired_route))
        {
            // Pre-existing routes remain owned by their creator, including an
            // identical route. Never try to replace or adopt them.
            continue;
        }
        let label = format!("{}/{}", desired_route.destination, desired_route.prefix);
        match backend.add(desired_route).await {
            Ok(()) => {
                added.push((label.clone(), desired_route.clone()));
                current.push(desired_route.clone());
                tracing::debug!(route = %label, ifindex = ?desired_route.ifindex, gateway = ?desired_route.gateway, "已添加路由");
            }
            Err(error) => {
                tracing::warn!(
                    route = %label,
                    error = %crate::error::redact_sensitive(&error.to_string()),
                    "添加路由失败，未记入所有权记录，等待重试"
                );
                let Some((old_label, old_route)) = removed_for_replacement
                    .iter()
                    .find(|(_, old)| same_prefix(old, desired_route))
                else {
                    continue;
                };
                // If switching a host route fails after removal, restore the
                // previous path in this round. Otherwise a sole /32 allowance
                // could unexpectedly fall through to the default route. A
                // revoked route has no replacement and is never restored.
                let Some(fresh) = list_routes(backend).await else {
                    return;
                };
                current = fresh;
                if current.iter().any(|route| same_prefix(route, old_route)) {
                    continue;
                }
                match backend.add(old_route).await {
                    Ok(()) => {
                        added.push((old_label.clone(), old_route.clone()));
                        current.push(old_route.clone());
                        tracing::warn!(route = %old_label, "新路径添加失败，已恢复原有路由，等待下次重试切换");
                    }
                    Err(error) => tracing::warn!(
                        route = %old_label,
                        error = %crate::error::redact_sensitive(&error.to_string()),
                        "恢复原有路由失败，等待下次重试"
                    ),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::VecDeque, sync::Mutex};

    #[derive(Default)]
    struct State {
        routes: Vec<Route>,
        calls: Vec<&'static str>,
        add_failures: usize,
        delete_failures: usize,
        disappear_on_delete_failure: bool,
        list_failures: VecDeque<bool>,
        list_replacements: VecDeque<Option<Vec<Route>>>,
    }

    struct FakeBackend {
        state: Mutex<State>,
        prefix_only_delete: bool,
    }

    impl FakeBackend {
        fn new(routes: Vec<Route>) -> Self {
            Self {
                state: Mutex::new(State {
                    routes,
                    ..State::default()
                }),
                prefix_only_delete: true,
            }
        }
    }

    #[async_trait]
    impl RouteBackend for FakeBackend {
        async fn list(&self) -> io::Result<Vec<Route>> {
            let mut state = self.state.lock().unwrap();
            state.calls.push("list");
            if state.list_failures.pop_front().unwrap_or(false) {
                return Err(io::Error::other("list failed"));
            }
            if let Some(Some(routes)) = state.list_replacements.pop_front() {
                state.routes = routes;
            }
            Ok(state.routes.clone())
        }

        async fn add(&self, route: &Route) -> io::Result<()> {
            let mut state = self.state.lock().unwrap();
            state.calls.push("add");
            if state.add_failures > 0 {
                state.add_failures -= 1;
                return Err(io::Error::other("add failed"));
            }
            state.routes.push(route.clone());
            Ok(())
        }

        async fn delete(&self, route: &Route) -> io::Result<()> {
            let mut state = self.state.lock().unwrap();
            state.calls.push("delete");
            if state.delete_failures > 0 {
                state.delete_failures -= 1;
                if state.disappear_on_delete_failure {
                    state.routes.retain(|existing| !same_route(existing, route));
                }
                return Err(io::Error::other("delete failed"));
            }
            let index = state
                .routes
                .iter()
                .position(|existing| {
                    if self.prefix_only_delete {
                        same_prefix(existing, route)
                    } else {
                        same_route(existing, route)
                    }
                })
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
            // Assert the actual observed row is forwarded (metric/LUID may
            // differ from the original add request).
            assert_eq!(&state.routes[index], route);
            state.routes.remove(index);
            Ok(())
        }

        fn deletion_requires_unique_prefix(&self) -> bool {
            self.prefix_only_delete
        }
    }

    fn tunnel() -> Route {
        Route::new("192.168.188.111".parse().unwrap(), 32).with_ifindex(99)
    }

    fn physical() -> Route {
        Route::new("192.168.188.111".parse().unwrap(), 32)
            .with_ifindex(4)
            .with_gateway("192.168.187.1".parse().unwrap())
    }

    fn ledger(route: Route) -> Vec<(String, Route)> {
        vec![(format!("{}/{}", route.destination, route.prefix), route)]
    }

    #[tokio::test]
    async fn switches_same_host_between_tunnel_and_physical_gateway() {
        let backend = FakeBackend::new(vec![tunnel()]);
        let mut added = ledger(tunnel());
        reconcile_with(&backend, &mut added, &[physical()]).await;
        assert_eq!(added, ledger(physical()));
        assert_eq!(backend.state.lock().unwrap().routes, vec![physical()]);
        reconcile_with(&backend, &mut added, &[tunnel()]).await;
        assert_eq!(added, ledger(tunnel()));
        assert_eq!(backend.state.lock().unwrap().routes, vec![tunnel()]);
    }

    #[tokio::test]
    async fn failed_delete_blocks_replacement_and_is_retried() {
        let backend = FakeBackend::new(vec![tunnel()]);
        backend.state.lock().unwrap().delete_failures = 1;
        let mut added = ledger(tunnel());
        reconcile_with(&backend, &mut added, &[physical()]).await;
        assert_eq!(added, ledger(tunnel()));
        assert!(!backend.state.lock().unwrap().calls.contains(&"add"));
        reconcile_with(&backend, &mut added, &[physical()]).await;
        assert_eq!(added, ledger(physical()));
    }

    #[tokio::test]
    async fn missing_owned_route_is_reinstalled_without_deleting() {
        let backend = FakeBackend::new(vec![]);
        let mut added = ledger(tunnel());
        reconcile_with(&backend, &mut added, &[tunnel()]).await;
        assert_eq!(added, ledger(tunnel()));
        assert!(!backend.state.lock().unwrap().calls.contains(&"delete"));
        assert_eq!(backend.state.lock().unwrap().routes, vec![tunnel()]);
    }

    #[tokio::test]
    async fn confirmed_disappearance_after_delete_failure_releases_ownership() {
        let backend = FakeBackend::new(vec![tunnel()]);
        {
            let mut state = backend.state.lock().unwrap();
            state.delete_failures = 1;
            state.disappear_on_delete_failure = true;
        }
        let mut added = ledger(tunnel());
        reconcile_with(&backend, &mut added, &[physical()]).await;
        assert_eq!(added, ledger(physical()));
    }

    #[tokio::test]
    async fn third_party_replacement_is_neither_deleted_nor_adopted() {
        let backend = FakeBackend::new(vec![physical()]);
        let mut added = ledger(tunnel());
        reconcile_with(&backend, &mut added, &[physical()]).await;
        assert!(added.is_empty());
        reconcile_with(&backend, &mut added, &[]).await;
        let state = backend.state.lock().unwrap();
        assert_eq!(state.routes, vec![physical()]);
        assert!(!state.calls.contains(&"delete"));
        assert!(!state.calls.contains(&"add"));
    }

    #[tokio::test]
    async fn prefix_only_delete_preserves_coexisting_foreign_route() {
        let backend = FakeBackend::new(vec![physical(), tunnel()]);
        let mut added = ledger(tunnel());
        reconcile_with(&backend, &mut added, &[]).await;
        assert_eq!(added, ledger(tunnel()));
        assert!(!backend.state.lock().unwrap().calls.contains(&"delete"));
    }

    #[tokio::test]
    async fn targeted_delete_preserves_coexisting_foreign_route() {
        let mut backend = FakeBackend::new(vec![physical(), tunnel()]);
        backend.prefix_only_delete = false;
        let mut added = ledger(tunnel());
        reconcile_with(&backend, &mut added, &[]).await;
        assert!(added.is_empty());
        assert_eq!(backend.state.lock().unwrap().routes, vec![physical()]);
    }

    #[tokio::test]
    async fn failed_add_is_not_owned_and_is_retried() {
        let backend = FakeBackend::new(vec![]);
        backend.state.lock().unwrap().add_failures = 1;
        let mut added = vec![];
        reconcile_with(&backend, &mut added, &[physical()]).await;
        assert!(added.is_empty());
        reconcile_with(&backend, &mut added, &[physical()]).await;
        assert_eq!(added, ledger(physical()));
    }

    #[tokio::test]
    async fn failed_replacement_restores_old_path_and_retries_next_round() {
        let backend = FakeBackend::new(vec![tunnel()]);
        backend.state.lock().unwrap().add_failures = 1;
        let mut added = ledger(tunnel());
        reconcile_with(&backend, &mut added, &[physical()]).await;
        assert_eq!(added, ledger(tunnel()));
        assert_eq!(backend.state.lock().unwrap().routes, vec![tunnel()]);
        reconcile_with(&backend, &mut added, &[physical()]).await;
        assert_eq!(added, ledger(physical()));
        assert_eq!(backend.state.lock().unwrap().routes, vec![physical()]);
    }

    #[tokio::test]
    async fn failed_restore_is_not_owned_and_next_round_can_retry() {
        let backend = FakeBackend::new(vec![tunnel()]);
        backend.state.lock().unwrap().add_failures = 2;
        let mut added = ledger(tunnel());
        reconcile_with(&backend, &mut added, &[physical()]).await;
        assert!(added.is_empty());
        assert!(backend.state.lock().unwrap().routes.is_empty());
        reconcile_with(&backend, &mut added, &[physical()]).await;
        assert_eq!(added, ledger(physical()));
    }

    #[tokio::test]
    async fn failed_replacement_does_not_restore_over_new_foreign_route() {
        let backend = FakeBackend::new(vec![tunnel()]);
        {
            let mut state = backend.state.lock().unwrap();
            state.add_failures = 1;
            state.list_replacements = VecDeque::from([None, None, None, Some(vec![physical()])]);
        }
        let mut added = ledger(tunnel());
        reconcile_with(&backend, &mut added, &[physical()]).await;
        assert!(added.is_empty());
        let state = backend.state.lock().unwrap();
        assert_eq!(state.routes, vec![physical()]);
        assert_eq!(state.calls.iter().filter(|call| **call == "add").count(), 1);
    }

    #[tokio::test]
    async fn revoked_route_is_not_restored_when_unrelated_add_fails() {
        let backend = FakeBackend::new(vec![tunnel()]);
        backend.state.lock().unwrap().add_failures = 1;
        let mut added = ledger(tunnel());
        let unrelated = Route::new("10.20.30.0".parse().unwrap(), 24).with_ifindex(99);
        reconcile_with(&backend, &mut added, &[unrelated]).await;
        assert!(added.is_empty());
        assert!(backend.state.lock().unwrap().routes.is_empty());
    }

    #[tokio::test]
    async fn replacement_between_snapshot_and_delete_is_preserved() {
        let backend = FakeBackend::new(vec![tunnel()]);
        backend.state.lock().unwrap().list_replacements =
            VecDeque::from([None, Some(vec![physical()])]);
        let mut added = ledger(tunnel());
        reconcile_with(&backend, &mut added, &[]).await;
        assert!(added.is_empty());
        let state = backend.state.lock().unwrap();
        assert_eq!(state.routes, vec![physical()]);
        assert!(!state.calls.contains(&"delete"));
    }

    #[tokio::test]
    async fn existing_identical_route_is_not_owned_or_deleted() {
        let backend = FakeBackend::new(vec![tunnel()]);
        let mut added = vec![];
        reconcile_with(&backend, &mut added, &[tunnel()]).await;
        reconcile_with(&backend, &mut added, &[]).await;
        assert!(added.is_empty());
        assert_eq!(backend.state.lock().unwrap().routes, vec![tunnel()]);
    }

    #[tokio::test]
    async fn list_failure_preserves_ledger_and_system_routes() {
        let backend = FakeBackend::new(vec![tunnel()]);
        backend.state.lock().unwrap().list_failures.push_back(true);
        let mut added = ledger(tunnel());
        reconcile_with(&backend, &mut added, &[physical()]).await;
        assert_eq!(added, ledger(tunnel()));
        assert_eq!(backend.state.lock().unwrap().calls, vec!["list"]);
    }

    #[tokio::test]
    async fn failed_verification_after_failed_delete_preserves_ledger() {
        let backend = FakeBackend::new(vec![tunnel()]);
        {
            let mut state = backend.state.lock().unwrap();
            state.delete_failures = 1;
            state.list_failures = VecDeque::from([false, false, true]);
        }
        let mut added = ledger(tunnel());
        reconcile_with(&backend, &mut added, &[physical()]).await;
        assert_eq!(added, ledger(tunnel()));
        assert!(!backend.state.lock().unwrap().calls.contains(&"add"));
    }

    #[tokio::test]
    async fn normalized_on_link_gateway_is_recognized_and_deleted_using_observed_row() {
        let observed = tunnel().with_gateway("0.0.0.0".parse().unwrap());
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        let observed = observed.with_metric(0);
        #[cfg(target_os = "windows")]
        let observed = observed.with_luid(42);
        let backend = FakeBackend::new(vec![observed]);
        let mut added = ledger(tunnel());
        reconcile_with(&backend, &mut added, &[]).await;
        assert!(added.is_empty());
        assert!(backend.state.lock().unwrap().routes.is_empty());
    }

    #[tokio::test]
    async fn cleanup_retries_failures_without_losing_ledger() {
        let backend = FakeBackend::new(vec![tunnel()]);
        backend.state.lock().unwrap().delete_failures = 1;
        let mut added = ledger(tunnel());
        reconcile_with(&backend, &mut added, &[]).await;
        assert_eq!(added.len(), 1);
        reconcile_with(&backend, &mut added, &[]).await;
        assert!(added.is_empty());
    }

    #[test]
    fn gateway_and_interface_are_part_of_route_identity() {
        assert!(!same_route(&tunnel(), &physical()));
        assert!(!same_route(&physical(), &physical().with_ifindex(5)));
        assert!(!same_route(
            &physical(),
            &physical().with_gateway("192.168.187.2".parse().unwrap())
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_tables_have_separate_ownership() {
        assert!(!same_route(&tunnel(), &tunnel().with_table(100)));
    }
}
