//! Lightweight domain event bus service.

use serde::Serialize;
use uuid::Uuid;
use vpn_core::{AppError, Result};

use crate::repositories::SqliteDomainEventRepository;

pub const EVENT_GATEWAY_OFFLINE: &str = "gateway_offline";
pub const EVENT_GATEWAY_RECOVERED: &str = "gateway_recovered";

#[derive(Clone)]
pub struct DomainEventService {
    repo: SqliteDomainEventRepository,
}

impl DomainEventService {
    pub fn new(repo: SqliteDomainEventRepository) -> Self {
        Self { repo }
    }

    pub async fn publish<T: Serialize>(
        &self,
        event_type: &str,
        aggregate_type: &str,
        aggregate_id: &str,
        payload: &T,
    ) -> Result<()> {
        let body = serde_json::to_string(payload).map_err(|e| AppError::Internal(Box::new(e)))?;
        self.repo
            .insert(
                &Uuid::now_v7().to_string(),
                event_type,
                aggregate_type,
                aggregate_id,
                &body,
                chrono::Utc::now().timestamp_millis(),
            )
            .await
    }

    pub async fn publish_best_effort<T: Serialize>(
        &self,
        event_type: &str,
        aggregate_type: &str,
        aggregate_id: &str,
        payload: &T,
    ) {
        if let Err(e) = self
            .publish(event_type, aggregate_type, aggregate_id, payload)
            .await
        {
            tracing::warn!(
                event_type,
                aggregate_type,
                aggregate_id,
                error = %e,
                "领域事件写入失败（忽略）"
            );
        }
    }
}
