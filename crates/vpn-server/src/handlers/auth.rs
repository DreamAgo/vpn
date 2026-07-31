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

fn extract_client_info(headers: &HeaderMap) -> (Option<String>, Option<String>) {
    let ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(',').next().unwrap_or(s).trim().to_string());
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

#[tracing::instrument(skip(state))]
pub async fn feishu_start(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ApiResponse<FeishuAuthStartResponse>>, ApiError> {
    let (ip, _) = extract_client_info(&headers);
    let response = state
        .feishu_auth_service()?
        .start(ip.as_deref().unwrap_or("unknown"))
        .await?;
    Ok(success(&state, response))
}

#[derive(Debug, Deserialize)]
pub struct FeishuCallbackQuery {
    state: String,
    code: Option<String>,
    error: Option<String>,
}

const FEISHU_CALLBACK_SUCCESS_HTML: &str = r#"<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<title>授权成功</title>
</head>
<body>
<p>飞书授权成功。</p>
<p>如未自动关闭，请手动关闭此窗口并返回客户端。</p>
<script>
window.setTimeout(function () {
    window.close();
}, 1200);
</script>
</body>
</html>"#;

const FEISHU_CALLBACK_FAILURE_HTML: &str = r#"<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<title>授权失败</title>
</head>
<body>
<p>飞书授权失败或已过期，请关闭此窗口后在客户端重试。</p>
</body>
</html>"#;

fn feishu_callback_html(ok: bool) -> &'static str {
    if ok {
        FEISHU_CALLBACK_SUCCESS_HTML
    } else {
        FEISHU_CALLBACK_FAILURE_HTML
    }
}

/// 回调页只显示结果，不携带本站或飞书 token。
#[tracing::instrument(skip(state, query), fields(outcome))]
pub async fn feishu_callback(
    State(state): State<AppState>,
    Query(query): Query<FeishuCallbackQuery>,
) -> Html<&'static str> {
    let ok = match state.feishu_auth_service() {
        Ok(service) => service
            .callback(&query.state, query.code.as_deref(), query.error.as_deref())
            .await
            .is_ok(),
        Err(_) => false,
    };
    tracing::Span::current().record("outcome", if ok { "success" } else { "failed" });
    Html(feishu_callback_html(ok))
}

#[tracing::instrument(skip(state, headers, body))]
pub async fn feishu_poll(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<FeishuAuthPollRequest>,
) -> Result<Json<ApiResponse<FeishuAuthPollResponse>>, ApiError> {
    let (ip, ua) = extract_client_info(&headers);
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
    if completed && state.audit_service.is_some() {
        state
            .audit_service()?
            .log_external_login_attempt(
                "feishu",
                true,
                None,
                ip.as_deref(),
                ua.as_deref(),
                state.clock.now_unix_ms(),
            )
            .await;
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
    headers: HeaderMap,
    Json(body): Json<LoginRequest>,
) -> Result<Json<ApiResponse<LoginResponse>>, ApiError> {
    let svc = state.auth_service()?;
    let (ip, ua) = extract_client_info(&headers);
    let result = svc
        .login(&body.username, &body.password, ip.as_deref(), ua.as_deref())
        .await;

    // Story 5.2：登录成功/失败均写审计（尽力而为，不阻塞）。
    if let Ok(audit) = state.audit_service() {
        let now = state.clock.now_unix_ms();
        match &result {
            Ok(_) => {
                audit
                    .log_login_attempt(&body.username, true, None, ip.as_deref(), now)
                    .await
            }
            Err(e) => {
                audit
                    .log_login_attempt(
                        &body.username,
                        false,
                        Some(&e.to_string()),
                        ip.as_deref(),
                        now,
                    )
                    .await
            }
        }
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
    use super::{feishu_callback_html, FEISHU_CALLBACK_FAILURE_HTML, FEISHU_CALLBACK_SUCCESS_HTML};

    #[test]
    fn feishu_callback_success_page_attempts_close_with_visible_fallback() {
        assert!(FEISHU_CALLBACK_SUCCESS_HTML
            .contains("window.setTimeout(function () {\n    window.close();\n}, 1200);"));
        assert!(
            FEISHU_CALLBACK_SUCCESS_HTML.contains("如未自动关闭，请手动关闭此窗口并返回客户端。")
        );
    }

    #[test]
    fn feishu_callback_failure_page_remains_visible() {
        assert!(FEISHU_CALLBACK_FAILURE_HTML.contains("授权失败"));
        assert!(FEISHU_CALLBACK_FAILURE_HTML.contains("在客户端重试"));
        assert!(!FEISHU_CALLBACK_FAILURE_HTML.contains("window.close"));
        assert!(!FEISHU_CALLBACK_FAILURE_HTML.contains("window.setTimeout"));
    }

    #[test]
    fn feishu_callback_result_selects_matching_page() {
        assert_eq!(feishu_callback_html(true), FEISHU_CALLBACK_SUCCESS_HTML);
        assert_eq!(feishu_callback_html(false), FEISHU_CALLBACK_FAILURE_HTML);
    }
}
