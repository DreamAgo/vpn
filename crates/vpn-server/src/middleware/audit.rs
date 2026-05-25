//! 审计中间件（Story 5.2）：对写操作（POST/PATCH/DELETE）在响应完成后异步写 audit_logs。
//!
//! 层序：必须包裹在认证中间件**内层**（即在 `require_auth` 之后运行），
//! 这样 request.extensions 里才有 `CurrentUser`。见 app.rs 的 ServiceBuilder 装配。

use axum::{
    extract::{Request, State},
    http::{header::USER_AGENT, Method},
    middleware::Next,
    response::Response,
};

use crate::{
    auth::CurrentUser, repositories::AuditLogEntry, services::infer_action, state::AppState,
};

/// 从请求头提取客户端 IP（优先 x-forwarded-for 第一段）。
fn extract_ip(req: &Request) -> Option<String> {
    req.headers()
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(',').next().unwrap_or(s).trim().to_string())
        .filter(|s| !s.is_empty())
}

fn extract_ua(req: &Request) -> Option<String> {
    req.headers()
        .get(USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

/// 仅对写操作记录审计。
fn is_audited_method(method: &Method) -> bool {
    matches!(method, &Method::POST | &Method::PATCH | &Method::DELETE)
}

/// 审计中间件。对写操作在响应后写一条审计日志（失败降级，不阻塞）。
pub async fn audit_layer(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let method = req.method().clone();
    if !is_audited_method(&method) {
        return next.run(req).await;
    }

    // 请求阶段先把要用的信息抓出来（响应阶段 req 已被消费）。
    let path = req.uri().path().to_string();
    let ip = extract_ip(&req);
    let user_agent = extract_ua(&req);
    let current = req.extensions().get::<CurrentUser>().cloned();

    let response = next.run(req).await;
    let status_code = response.status().as_u16() as i32;

    // 仅当 audit_service 已装配时记录（健康检查等最小装配场景跳过）。
    if let Some(audit) = state.audit_service.clone() {
        let action = infer_action(method.as_str(), &path);
        let (user_id, username) = match current {
            Some(u) => (Some(u.user_id), None),
            None => (None, None),
        };
        let entry = AuditLogEntry {
            user_id,
            username,
            action,
            resource: path,
            ip_addr: ip,
            user_agent,
            metadata: None,
            status_code: Some(status_code),
        };
        let now = state.clock.now_unix_ms();
        // 异步写入：spawn 出去不阻塞响应返回。
        tokio::spawn(async move {
            audit.log(entry, now).await;
        });
    }

    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audited_methods_only_writes() {
        assert!(is_audited_method(&Method::POST));
        assert!(is_audited_method(&Method::PATCH));
        assert!(is_audited_method(&Method::DELETE));
        assert!(!is_audited_method(&Method::GET));
        assert!(!is_audited_method(&Method::HEAD));
    }
}
