//! 飞书审批 webhook：服务层完成验签/解密/落库后快速应答。

use axum::{body::Bytes, extract::State, http::HeaderMap, response::IntoResponse, Json};
use serde_json::json;

use crate::{
    error::ApiError,
    services::{ApprovalEventHeaders, ApprovalWebhookReply},
    AppState,
};

pub async fn webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl IntoResponse, ApiError> {
    let reply = state
        .feishu_approval_service()?
        .receive(
            ApprovalEventHeaders {
                timestamp: optional_header(&headers, "x-lark-request-timestamp"),
                nonce: optional_header(&headers, "x-lark-request-nonce"),
                signature: optional_header(&headers, "x-lark-signature"),
            },
            &body,
        )
        .await?;
    Ok(match reply {
        ApprovalWebhookReply::Ack => Json(json!({"code": 0})),
        ApprovalWebhookReply::Challenge(challenge) => Json(json!({"challenge": challenge})),
    })
}

fn optional_header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
}
