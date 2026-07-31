//! 飞书审批“关联外部选项”HTTP 适配层。

use axum::{
    extract::{rejection::JsonRejection, Path, State},
    http::StatusCode,
    Json,
};
use vpn_api_types::external_options::{ExternalOptionsRequest, ExternalOptionsResponse};

use crate::{services::ExternalOptionsError, state::AppState};

#[tracing::instrument(skip(state, body), fields(source = %source))]
pub async fn list_external_options(
    State(state): State<AppState>,
    Path(source): Path<String>,
    body: Result<Json<ExternalOptionsRequest>, JsonRejection>,
) -> (StatusCode, Json<ExternalOptionsResponse>) {
    let Json(body) = match body {
        Ok(body) => body,
        Err(_) => return error_response(ExternalOptionsError::InvalidRequest),
    };
    let Some(service) = state.external_options_service.as_ref() else {
        return error_response(ExternalOptionsError::NotConfigured);
    };

    match tokio::time::timeout(
        std::time::Duration::from_millis(2_500),
        service.query(&source, &body),
    )
    .await
    {
        Err(_) => error_response(ExternalOptionsError::Timeout),
        Ok(result) => match result {
            Ok(result) => (
                StatusCode::OK,
                Json(ExternalOptionsResponse::success(result)),
            ),
            Err(error) => {
                if let ExternalOptionsError::Backend(source) = &error {
                    tracing::error!(error = ?source, "读取飞书审批外部选项失败");
                }
                error_response(error)
            }
        },
    }
}

fn error_response(error: ExternalOptionsError) -> (StatusCode, Json<ExternalOptionsResponse>) {
    let (status, code) = match error {
        ExternalOptionsError::NotConfigured => (StatusCode::SERVICE_UNAVAILABLE, 50300),
        ExternalOptionsError::Unauthorized => (StatusCode::UNAUTHORIZED, 40100),
        ExternalOptionsError::UnknownSource => (StatusCode::NOT_FOUND, 40400),
        ExternalOptionsError::InvalidCursor => (StatusCode::BAD_REQUEST, 40000),
        ExternalOptionsError::InvalidRequest => (StatusCode::BAD_REQUEST, 40001),
        ExternalOptionsError::Timeout => (StatusCode::GATEWAY_TIMEOUT, 50400),
        ExternalOptionsError::Backend(_) => (StatusCode::INTERNAL_SERVER_ERROR, 50000),
    };
    (
        status,
        Json(ExternalOptionsResponse::error(code, error.to_string())),
    )
}
