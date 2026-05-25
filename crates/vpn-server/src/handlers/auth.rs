//! 认证 API handler。
//!
//! 端点：
//! - POST /api/v1/auth/login
//! - POST /api/v1/auth/refresh
//! - POST /api/v1/auth/logout
//! - POST /api/v1/auth/change-password
//! - POST /api/v1/auth/first-time-setup

use axum::{extract::State, http::HeaderMap, Json};
use vpn_api_types::{
    auth::{
        ChangePasswordRequest, FirstTimeSetupRequest, FirstTimeSetupResponse, LoginRequest,
        LoginResponse, LogoutRequest, RefreshRequest, RefreshResponse, SetupStatusResponse,
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
