//! Service account API key management.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::Utc;
use rand::{rngs::OsRng, RngCore};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use vpn_api_types::api_key::{ApiKeyDto, CreateApiKeyResponse};
use vpn_core::{AppError, Result};

use crate::repositories::{ApiKeyRow, SqliteApiKeyRepository};

#[derive(Debug, Clone)]
pub struct VerifiedApiKey {
    pub id: String,
    pub name: String,
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ApiKeyService {
    repo: SqliteApiKeyRepository,
}

impl ApiKeyService {
    pub fn new(repo: SqliteApiKeyRepository) -> Self {
        Self { repo }
    }

    pub async fn create(
        &self,
        name: &str,
        scopes: &[String],
        created_by: &str,
    ) -> Result<CreateApiKeyResponse> {
        let name = name.trim();
        if name.is_empty() {
            return Err(AppError::Validation("API Key 名称不能为空".to_string()));
        }

        let id = Uuid::now_v7().to_string();
        let secret = generate_secret();
        let key = format!("ylk_{}_{}", id, secret);
        let key_hash = hash_key(&key);
        let scopes = normalize_scopes(scopes);
        let scopes_json =
            serde_json::to_string(&scopes).map_err(|e| AppError::Internal(Box::new(e)))?;
        let now = Utc::now().timestamp_millis();

        self.repo
            .insert(&id, name, &key_hash, &scopes_json, created_by, now)
            .await?;

        Ok(CreateApiKeyResponse {
            api_key: ApiKeyDto {
                id,
                name: name.to_string(),
                scopes,
                status: "active".to_string(),
                created_by: created_by.to_string(),
                last_used_at: None,
                revoked_at: None,
                created_at: now,
            },
            key,
        })
    }

    pub async fn list(&self) -> Result<Vec<ApiKeyDto>> {
        Ok(self
            .repo
            .list()
            .await?
            .into_iter()
            .map(row_to_dto)
            .collect())
    }

    pub async fn revoke(&self, id: &str) -> Result<()> {
        let affected = self.repo.revoke(id, Utc::now().timestamp_millis()).await?;
        if affected == 0 {
            return Err(AppError::ResourceNotFound(
                "API Key 不存在或已吊销".to_string(),
            ));
        }
        Ok(())
    }

    pub async fn verify(&self, key: &str) -> Result<Option<VerifiedApiKey>> {
        if !key.starts_with("ylk_") {
            return Ok(None);
        }
        let key_hash = hash_key(key);
        let Some(row) = self.repo.find_active_by_hash(&key_hash).await? else {
            return Ok(None);
        };
        self.repo
            .touch_last_used(&row.id, Utc::now().timestamp_millis())
            .await?;
        Ok(Some(VerifiedApiKey {
            id: row.id,
            name: row.name,
            scopes: parse_scopes(&row.scopes),
        }))
    }
}

fn generate_secret() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn hash_key(key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn normalize_scopes(scopes: &[String]) -> Vec<String> {
    let mut v: Vec<String> = scopes
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    if v.is_empty() {
        v.push("admin:*".to_string());
    }
    v.sort();
    v.dedup();
    v
}

fn parse_scopes(raw: &str) -> Vec<String> {
    serde_json::from_str(raw).unwrap_or_else(|_| {
        raw.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
            .collect()
    })
}

fn row_to_dto(row: ApiKeyRow) -> ApiKeyDto {
    ApiKeyDto {
        id: row.id,
        name: row.name,
        scopes: parse_scopes(&row.scopes),
        status: row.status,
        created_by: row.created_by,
        last_used_at: row.last_used_at,
        revoked_at: row.revoked_at,
        created_at: row.created_at,
    }
}
