//! Administrative request auditing. Business changes are recorded in their own transactions.
use super::audit_context::{AuditContext, CONTEXT};
use crate::{
    auth::CurrentUser, repositories::AuditLogEntry, services::infer_action, state::AppState,
};
use axum::{
    extract::{ConnectInfo, Request, State},
    http::{header::USER_AGENT, Method},
    middleware::Next,
    response::Response,
};
use std::{
    net::{IpAddr, SocketAddr},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

pub fn client_ip(
    headers: &axum::http::HeaderMap,
    peer: Option<SocketAddr>,
    trusted: &[IpAddr],
) -> Option<String> {
    let peer = peer?.ip();
    if trusted.contains(&peer) {
        // Walk right to left: the first non-proxy hop is authoritative.
        if let Some(raw) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
            let hops: Option<Vec<IpAddr>> = raw.split(',').map(|v| v.trim().parse().ok()).collect();
            if let Some(hops) = hops {
                return Some(
                    hops.into_iter()
                        .rev()
                        .find(|ip| !trusted.contains(ip))
                        .unwrap_or(peer)
                        .to_string(),
                );
            }
        }
    }
    Some(peer.to_string())
}
fn is_audited_method(method: &Method) -> bool {
    matches!(
        method,
        &Method::POST | &Method::PUT | &Method::PATCH | &Method::DELETE
    )
}

pub async fn audit_layer(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let path = req.uri().path().to_string();
    if (!is_audited_method(req.method()) && path != "/api/v1/admin/backup")
        || path == "/api/v1/peers/heartbeat"
    {
        return next.run(req).await;
    }
    let current = req.extensions().get::<CurrentUser>().cloned();
    let user_id = current.map(|u| u.user_id);
    let username = match (&state.audit_service, &user_id) {
        (Some(audit), Some(id)) => audit.actor_name(id).await,
        _ => None,
    };
    let request_id = req
        .extensions()
        .get::<tower_http::request_id::RequestId>()
        .and_then(|id| id.header_value().to_str().ok())
        .map(|id| id.chars().take(128).collect())
        .unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
    let entry = AuditLogEntry {
        user_id,
        username,
        action: infer_action(req.method().as_str(), &path),
        resource: path,
        ip_addr: client_ip(
            req.headers(),
            req.extensions()
                .get::<ConnectInfo<SocketAddr>>()
                .map(|c| c.0),
            &state.trusted_proxies,
        ),
        user_agent: req
            .headers()
            .get(USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.chars().take(512).collect()),
        metadata: None,
        status_code: None,
    };
    let context = AuditContext {
        entry: entry.clone(),
        request_id,
        committed: Arc::new(AtomicUsize::new(0)),
    };
    let mut response = CONTEXT.scope(context.clone(), next.run(req)).await;
    let status = response.status();
    response.headers_mut().insert(
        "x-request-id",
        context.request_id.parse().expect("UUID is a header value"),
    );
    let commits = context.committed.load(Ordering::Relaxed);
    if let Some(audit) = &state.audit_service {
        // A failed application step can follow a committed DB update; preserve both facts.
        if commits == 0 || !status.is_success() {
            let mut entry = entry;
            entry.status_code = Some(status.as_u16() as i32);
            entry.metadata = Some(
                serde_json::json!({"request_id":context.request_id,
                    "outcome":if status.is_success(){"success"}else{"failed"},
                    "committed_changes":commits,
                "reason_code":response.extensions().get::<crate::error::AuditFailure>().map(|e|e.0),
                    "reason":if status.is_success(){None}else{status.canonical_reason()}
                })
                .to_string(),
            );
            audit.log(entry, state.clock.now_unix_ms()).await;
        }
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn put_is_audited() {
        assert!(is_audited_method(&Method::PUT));
        assert!(!is_audited_method(&Method::GET));
    }
    #[test]
    fn forwarded_ip_requires_trusted_transport_peer() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("x-forwarded-for", "1.2.3.4, 10.0.0.1".parse().unwrap());
        let peer = "10.0.0.2:1234".parse().unwrap();
        assert_eq!(
            client_ip(&headers, Some(peer), &[]).as_deref(),
            Some("10.0.0.2")
        );
        assert_eq!(
            client_ip(&headers, Some(peer), &["10.0.0.2".parse().unwrap()]).as_deref(),
            Some("10.0.0.1")
        );
        assert_eq!(client_ip(&headers, None, &[]), None);
    }
}
