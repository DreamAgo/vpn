//! Axum Router 工厂。

use axum::{
    middleware::from_fn_with_state,
    routing::{delete, get, patch, post},
    Router,
};
use tower::ServiceBuilder;
use tower_http::{
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    services::ServeDir,
    trace::TraceLayer,
};

use crate::{handlers, middleware, state::AppState};

const REQUEST_ID_HEADER: &str = "x-request-id";

/// 构造完整的 Axum Router（含中间件链）。
///
/// 中间件执行顺序（请求路径）：
/// 1. SetRequestIdLayer 注入 X-Request-Id（若客户端未带则生成 UUID v4）
/// 2. TraceLayer 记录结构化日志（请求/响应/耗时）
/// 3. handler 业务逻辑
/// 4. PropagateRequestIdLayer 把 request_id 复制到响应头
///
/// 认证路由（需要 JWT 的端点）会单独走 `middleware::require_auth`。
pub fn build_router(state: AppState) -> Router {
    let request_id_header = axum::http::HeaderName::from_static(REQUEST_ID_HEADER);

    let middleware = ServiceBuilder::new()
        .layer(SetRequestIdLayer::new(
            request_id_header.clone(),
            MakeRequestUuid,
        ))
        .layer(TraceLayer::new_for_http())
        .layer(PropagateRequestIdLayer::new(request_id_header));

    // 公开路由（无需认证）
    let public_routes = Router::new()
        .route("/health", get(handlers::health::health_handler))
        .route("/api/v1/openapi.json", get(handlers::openapi::openapi_json))
        .route(
            "/api/v1/auth/setup-status",
            get(handlers::auth::setup_status),
        )
        .route(
            "/api/v1/auth/first-time-setup",
            post(handlers::auth::first_time_setup),
        )
        .route("/api/v1/auth/login", post(handlers::auth::login))
        .route("/api/v1/auth/refresh", post(handlers::auth::refresh));

    // 认证路由（需要 JWT）
    let authed_routes = if state.auth_service.is_some() {
        Router::new()
            .route("/api/v1/auth/logout", post(handlers::auth::logout))
            .route(
                "/api/v1/auth/change-password",
                post(handlers::auth::change_password),
            )
            .route(
                "/api/v1/admin/system/info",
                get(handlers::system::system_info),
            )
            .route(
                "/api/v1/admin/system/routes",
                axum::routing::put(handlers::system::update_server_routes),
            )
            .route(
                "/api/v1/admin/notifications/email",
                get(handlers::system::email_notification_settings)
                    .put(handlers::system::update_email_notification_settings),
            )
            .route(
                "/api/v1/admin/notifications/email/test",
                post(handlers::system::test_email_notification),
            )
            .route(
                "/api/v1/admin/notifications/events",
                get(handlers::system::list_notification_events),
            )
            .route(
                "/api/v1/admin/backup",
                get(handlers::backup::download_backup),
            )
            .route(
                "/api/v1/admin/backup/restore",
                post(handlers::backup::restore_backup),
            )
            .route(
                "/api/v1/admin/users",
                post(handlers::users::create_user).get(handlers::users::list_users),
            )
            .route(
                "/api/v1/admin/users/{id}",
                patch(handlers::users::update_user).delete(handlers::users::delete_user),
            )
            .route(
                "/api/v1/admin/users/{id}/reset-password",
                post(handlers::users::reset_password),
            )
            .route(
                "/api/v1/admin/users/{id}/groups",
                axum::routing::put(handlers::users::set_user_groups),
            )
            .route(
                "/api/v1/admin/groups",
                get(handlers::groups::list_groups).post(handlers::groups::create_group),
            )
            .route(
                "/api/v1/admin/groups/{id}",
                patch(handlers::groups::update_group).delete(handlers::groups::delete_group),
            )
            .route(
                "/api/v1/admin/subnets",
                get(handlers::subnets::list_subnets).post(handlers::subnets::create_subnet),
            )
            .route(
                "/api/v1/admin/subnets/{id}",
                patch(handlers::subnets::update_subnet).delete(handlers::subnets::delete_subnet),
            )
            .route("/api/v1/peers/register", post(handlers::peers::register))
            .route("/api/v1/peers/heartbeat", post(handlers::peers::heartbeat))
            .route("/api/v1/peers/me", delete(handlers::peers::delete_me))
            .route(
                "/api/v1/peers/me/config",
                get(handlers::peers::download_config),
            )
            .route(
                "/api/v1/admin/audit-logs",
                get(handlers::audit::list_audit_logs),
            )
            .route(
                "/api/v1/admin/api-keys",
                get(handlers::api_keys::list_api_keys).post(handlers::api_keys::create_api_key),
            )
            .route(
                "/api/v1/admin/api-keys/{id}",
                delete(handlers::api_keys::revoke_api_key),
            )
            .route(
                "/api/v1/admin/peers",
                get(handlers::peers::list_admin_peers),
            )
            .route(
                "/api/v1/admin/peers/{id}",
                delete(handlers::peers::force_remove_peer)
                    .patch(handlers::peers::update_peer_routes),
            )
            .route(
                "/api/v1/admin/peers/{id}/purge",
                delete(handlers::peers::purge_peer),
            )
            .route(
                "/api/v1/admin/peer-events",
                get(handlers::peers::list_peer_events),
            )
            // 审计中间件（内层）：在 require_auth 之后运行，故 extensions 已含 CurrentUser。
            .layer(from_fn_with_state(state.clone(), middleware::audit_layer))
            .layer(from_fn_with_state(
                state.clone(),
                middleware::auth::require_auth,
            ))
    } else {
        // AuthService 未初始化（如健康检查测试场景）：不注册认证路由
        Router::new()
    };

    // 自动更新静态目录（VPN_DATA_DIR/updates），公开免鉴权：Tauri updater 拉 latest.json + 更新包。
    let updates_dir = format!(
        "{}/updates",
        std::env::var("VPN_DATA_DIR").unwrap_or_else(|_| "./data".to_string())
    );
    let _ = std::fs::create_dir_all(&updates_dir);

    Router::new()
        .merge(public_routes)
        .merge(authed_routes)
        .nest_service("/updates", ServeDir::new(&updates_dir))
        .with_state(state)
        .fallback(handlers::static_files::static_handler)
        .layer(middleware)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    #[tokio::test]
    async fn health_endpoint_returns_200() {
        let app = build_router(AppState::new());
        let req = Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn openapi_endpoint_returns_200() {
        let app = build_router(AppState::new());
        let req = Request::builder()
            .uri("/api/v1/openapi.json")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn request_id_header_propagated() {
        let app = build_router(AppState::new());
        let req = Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap();
        let response = app.oneshot(req).await.unwrap();
        assert!(response.headers().contains_key("x-request-id"));
    }
}
