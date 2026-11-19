use axum::{
    extract::{Query, State},
    Json,
};
use serde::Deserialize;
use vpn_api_types::{traffic::TrafficDashboard, ApiResponse};
use vpn_core::AppError;

use crate::{auth::RequireAdmin, error::ApiError, state::AppState};

#[derive(Default, Deserialize)]
pub struct TrafficQuery {
    days: Option<u32>,
}

pub async fn dashboard(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
    Query(query): Query<TrafficQuery>,
) -> Result<Json<ApiResponse<TrafficDashboard>>, ApiError> {
    let service = state
        .traffic
        .as_ref()
        .ok_or_else(|| AppError::Config("流量采集服务未初始化".into()))?;
    let sources = state
        .cluster
        .as_ref()
        .map(|cluster| cluster.traffic_sources())
        .unwrap_or_else(|| vec![("local".into(), "内置网关".into())]);
    let now = state.clock.now_unix_ms();
    let data = service
        .dashboard(query.days.unwrap_or(1), sources, now)
        .await?;
    Ok(Json(ApiResponse::success(data, "n/a".into(), now)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::CurrentUser;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        routing::get,
        Router,
    };
    use tower::ServiceExt;

    #[tokio::test]
    async fn traffic_is_admin_only_and_rejects_invalid_ranges() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
        let mut state = AppState::new();
        state.traffic = Some(std::sync::Arc::new(
            crate::services::traffic_service::TrafficService::new(pool),
        ));
        let app = Router::new()
            .route("/traffic", get(dashboard))
            .with_state(state);
        for (role, path, expected) in [
            (None, "/traffic", StatusCode::UNAUTHORIZED),
            (Some("user"), "/traffic", StatusCode::FORBIDDEN),
            (Some("admin"), "/traffic", StatusCode::OK),
            (Some("admin"), "/traffic?days=366", StatusCode::BAD_REQUEST),
        ] {
            let mut request = Request::builder().uri(path).body(Body::empty()).unwrap();
            if let Some(role) = role {
                request.extensions_mut().insert(CurrentUser {
                    user_id: "test".into(),
                    role: role.into(),
                });
            }
            let response = app.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), expected, "{role:?} {path}");
        }
    }
}
