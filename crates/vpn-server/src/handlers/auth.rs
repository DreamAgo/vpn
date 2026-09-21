//! 认证 API handler。
//!
//! 端点：
//! - POST /api/v1/auth/login
//! - POST /api/v1/auth/refresh
//! - POST /api/v1/auth/logout
//! - POST /api/v1/auth/change-password
//! - POST /api/v1/auth/first-time-setup

use axum::{
    extract::{Query, State},
    http::HeaderMap,
    response::Html,
    Json,
};
use serde::Deserialize;
use vpn_api_types::{
    auth::{
        ChangePasswordRequest, FeishuAuthConfigResponse, FeishuAuthPollRequest,
        FeishuAuthPollResponse, FeishuAuthStartResponse, FirstTimeSetupRequest,
        FirstTimeSetupResponse, LoginRequest, LoginResponse, LogoutRequest, RefreshRequest,
        RefreshResponse, SetupStatusResponse,
    },
    ApiResponse,
};
use vpn_core::service::PasswordHasher;

use crate::{auth::CurrentUser, error::ApiError, services::AuthService, state::AppState};

fn extract_client_info(
    headers: &HeaderMap,
    peer: Option<axum::Extension<axum::extract::ConnectInfo<std::net::SocketAddr>>>,
    state: &AppState,
) -> (Option<String>, Option<String>) {
    let ip =
        crate::middleware::audit::client_ip(headers, peer.map(|p| p.0 .0), &state.trusted_proxies);
    let ua = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    (ip, ua)
}

fn success<T: serde::Serialize>(state: &AppState, data: T) -> Json<ApiResponse<T>> {
    Json(ApiResponse::success(
        data,
        "n/a".to_string(),
        state.clock.now_unix_ms(),
    ))
}

#[tracing::instrument(skip(state))]
pub async fn feishu_config(
    State(state): State<AppState>,
) -> Json<ApiResponse<FeishuAuthConfigResponse>> {
    let enabled = state
        .feishu_auth_service
        .as_ref()
        .is_some_and(|svc| svc.enabled());
    success(&state, FeishuAuthConfigResponse { enabled })
}

#[derive(Debug, Default, Deserialize)]
pub struct FeishuStartQuery {
    client: Option<String>,
}

#[tracing::instrument(skip(state))]
pub async fn feishu_start(
    State(state): State<AppState>,
    Query(query): Query<FeishuStartQuery>,
    peer: Option<axum::Extension<axum::extract::ConnectInfo<std::net::SocketAddr>>>,
    headers: HeaderMap,
) -> Result<Json<ApiResponse<FeishuAuthStartResponse>>, ApiError> {
    let (ip, _) = extract_client_info(&headers, peer, &state);
    let response = state
        .feishu_auth_service()?
        .start_for_client(
            ip.as_deref().unwrap_or("unknown"),
            query.client.as_deref() == Some("android"),
        )
        .await?;
    Ok(success(&state, response))
}

#[derive(Debug, Deserialize)]
pub struct FeishuCallbackQuery {
    state: String,
    code: Option<String>,
    error: Option<String>,
}

fn feishu_callback_html(ok: bool, android: bool) -> String {
    include_str!("feishu_callback.html")
        .replace("{{RESULT}}", if ok { "success" } else { "failure" })
        .replace("{{ANDROID}}", if android { "true" } else { "false" })
        .replace(
            "{{TITLE}}",
            if ok {
                "飞书授权成功"
            } else {
                "未能完成授权"
            },
        )
        .replace(
            "{{MESSAGE}}",
            if ok {
                "授权结果已送达，请返回易链完成登录。"
            } else {
                "授权已取消、失效或暂时不可用，请返回易链重新发起登录。"
            },
        )
}

/// 回调页只显示结果，不携带本站或飞书 token。
#[tracing::instrument(skip(state, query), fields(outcome))]
pub async fn feishu_callback(
    State(state): State<AppState>,
    Query(query): Query<FeishuCallbackQuery>,
) -> impl axum::response::IntoResponse {
    let outcome = match state.feishu_auth_service() {
        Ok(service) => {
            service
                .callback(&query.state, query.code.as_deref(), query.error.as_deref())
                .await
        }
        Err(error) => Err(error),
    };
    let ok = outcome.is_ok();
    let android = outcome.unwrap_or(false);
    tracing::Span::current().record("outcome", if ok { "success" } else { "failed" });
    (
        [
            ("cache-control", "no-store"),
            ("referrer-policy", "no-referrer"),
        ],
        Html(feishu_callback_html(ok, android)),
    )
}

#[tracing::instrument(skip(state, headers, body))]
pub async fn feishu_poll(
    State(state): State<AppState>,
    peer: Option<axum::Extension<axum::extract::ConnectInfo<std::net::SocketAddr>>>,
    headers: HeaderMap,
    Json(body): Json<FeishuAuthPollRequest>,
) -> Result<Json<ApiResponse<FeishuAuthPollResponse>>, ApiError> {
    let (ip, ua) = extract_client_info(&headers, peer, &state);
    let result = state
        .feishu_auth_service()?
        .poll(&body.poll_token, ip.as_deref(), ua.as_deref())
        .await;
    let completed = result.as_ref().is_ok_and(|response| {
        matches!(
            response.status,
            vpn_api_types::auth::FeishuAuthPollStatus::Complete
        )
    });
    if completed || result.is_err() {
        if let Some(audit) = &state.audit_service {
            let username = result.as_ref().ok().and_then(|r| r.username.clone());
            let user_id = if let Some(name) = &username {
                state
                    .auth_service()?
                    .user_repo
                    .find_by_username(name)
                    .await?
                    .map(|u| u.id)
            } else {
                None
            };
            audit
                .log_auth(
                    crate::repositories::AuditLogEntry {
                        user_id,
                        username,
                        action: if completed {
                            "external_login_success"
                        } else {
                            "external_login_failed"
                        }
                        .into(),
                        resource: "/api/v1/auth/feishu/poll".into(),
                        ip_addr: ip,
                        user_agent: ua,
                        ..Default::default()
                    },
                    completed,
                    result.as_ref().err(),
                    headers.get("x-request-id").and_then(|v| v.to_str().ok()),
                    state.clock.now_unix_ms(),
                )
                .await;
        }
    }
    let response = result?;
    Ok(success(&state, response))
}

#[tracing::instrument(skip(state))]
pub async fn setup_status(
    State(state): State<AppState>,
) -> Result<Json<ApiResponse<SetupStatusResponse>>, ApiError> {
    let svc = state.auth_service()?;
    let admins = svc.user_repo.count_admins().await?;
    Ok(success(
        &state,
        SetupStatusResponse {
            needs_setup: admins == 0,
        },
    ))
}

#[tracing::instrument(skip(state, body))]
pub async fn first_time_setup(
    State(state): State<AppState>,
    Json(body): Json<FirstTimeSetupRequest>,
) -> Result<Json<ApiResponse<FirstTimeSetupResponse>>, ApiError> {
    let svc = state.auth_service()?;
    let outcome = svc
        .first_time_setup(&body.username, &body.email, &body.password)
        .await?;
    Ok(success(
        &state,
        FirstTimeSetupResponse {
            user_id: outcome.user.id,
            access_token: outcome.access_token,
            refresh_token: outcome.refresh_token,
        },
    ))
}

#[tracing::instrument(skip(state, headers, body))]
pub async fn login(
    State(state): State<AppState>,
    peer: Option<axum::Extension<axum::extract::ConnectInfo<std::net::SocketAddr>>>,
    headers: HeaderMap,
    Json(body): Json<LoginRequest>,
) -> Result<Json<ApiResponse<LoginResponse>>, ApiError> {
    let svc = state.auth_service()?;
    let (ip, ua) = extract_client_info(&headers, peer, &state);
    let result = svc
        .login(&body.username, &body.password, ip.as_deref(), ua.as_deref())
        .await;

    if let Some(audit) = &state.audit_service {
        audit
            .log_auth(
                crate::repositories::AuditLogEntry {
                    user_id: result.as_ref().ok().map(|r| r.user.id.clone()),
                    username: Some(body.username.chars().take(256).collect()),
                    action: if result.is_ok() {
                        "login_success"
                    } else {
                        "login_failed"
                    }
                    .into(),
                    resource: "/api/v1/auth/login".into(),
                    ip_addr: ip,
                    user_agent: ua,
                    ..Default::default()
                },
                result.is_ok(),
                result.as_ref().err(),
                headers.get("x-request-id").and_then(|v| v.to_str().ok()),
                state.clock.now_unix_ms(),
            )
            .await;
    }

    let outcome = result?;
    Ok(success(
        &state,
        LoginResponse {
            access_token: outcome.access_token,
            refresh_token: outcome.refresh_token,
            access_expires_in: AuthService::access_ttl_secs(),
            must_change_password: outcome.user.must_change_password,
        },
    ))
}

#[tracing::instrument(skip(state, body))]
pub async fn refresh(
    State(state): State<AppState>,
    Json(body): Json<RefreshRequest>,
) -> Result<Json<ApiResponse<RefreshResponse>>, ApiError> {
    let svc = state.auth_service()?;
    let access_token = svc.refresh(&body.refresh_token).await?;
    Ok(success(
        &state,
        RefreshResponse {
            access_token,
            access_expires_in: AuthService::access_ttl_secs(),
        },
    ))
}

#[tracing::instrument(skip(state, body))]
pub async fn logout(
    State(state): State<AppState>,
    Json(body): Json<LogoutRequest>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let svc = state.auth_service()?;
    svc.logout(&body.refresh_token).await?;
    Ok(success(&state, ()))
}

#[tracing::instrument(skip(state, body, current))]
pub async fn change_password(
    State(state): State<AppState>,
    current: CurrentUser,
    Json(body): Json<ChangePasswordRequest>,
) -> Result<Json<ApiResponse<()>>, ApiError> {
    let svc = state.auth_service()?;
    svc.change_password(&current.user_id, &body.old_password, &body.new_password)
        .await?;
    Ok(success(&state, ()))
}

// 让 dead_code lint 不抱怨：trait import 仅用于类型推导
#[allow(dead_code)]
fn _hasher_type_marker(_: Box<dyn PasswordHasher>) {}

#[cfg(test)]
mod tests {
    use super::feishu_callback_html;

    #[test]
    fn only_valid_android_success_auto_returns() {
        assert!(feishu_callback_html(true, true).contains("data-android=\"true\""));
        assert!(feishu_callback_html(true, false).contains("data-android=\"false\""));
        let failed = feishu_callback_html(false, false);
        assert!(failed.contains("data-result=\"failure\""));
        assert!(failed.contains("重新发起登录"));
    }

    #[test]
    fn result_pages_have_mobile_layout_and_no_oauth_parameters() {
        for ok in [true, false] {
            let page = feishu_callback_html(ok, false);
            assert!(page.contains("name=\"viewport\""));
            assert!(page.contains("返回易链"));
            assert!(page.contains("history.replaceState"));
            assert!(!page.contains("{{"));
            assert!(!page.contains("access_token"));
            assert!(!page.contains("poll_token"));
        }
    }
}
