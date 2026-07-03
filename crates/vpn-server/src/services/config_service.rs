//! Typed access layer for runtime configuration stored in `system_config`.

use vpn_core::Result;

use crate::repositories::SqliteSystemConfigRepository;

#[derive(Clone)]
pub struct ConfigService {
    repo: SqliteSystemConfigRepository,
}

impl ConfigService {
    pub fn new(repo: SqliteSystemConfigRepository) -> Self {
        Self { repo }
    }

    pub async fn get_raw(&self, key: &str) -> Result<Option<String>> {
        self.repo.get(key).await
    }

    pub async fn set_raw(&self, key: &str, value: &str) -> Result<()> {
        self.repo.set(key, value).await
    }

    pub async fn get_string(&self, key: &str) -> Result<Option<String>> {
        Ok(self.get_raw(key).await?.and_then(clean_string))
    }

    pub async fn set_string(&self, key: &str, value: Option<&str>) -> Result<()> {
        self.set_raw(key, value.unwrap_or("").trim()).await
    }

    pub async fn get_bool(&self, key: &str, default: bool) -> Result<bool> {
        Ok(self
            .get_raw(key)
            .await?
            .map(|v| parse_bool(&v))
            .unwrap_or(default))
    }

    pub async fn set_bool(&self, key: &str, value: bool) -> Result<()> {
        self.set_raw(key, if value { "true" } else { "false" })
            .await
    }

    pub async fn get_u16(&self, key: &str, default: u16) -> Result<u16> {
        Ok(self
            .get_raw(key)
            .await?
            .and_then(|v| v.parse::<u16>().ok())
            .unwrap_or(default))
    }

    pub async fn set_u16(&self, key: &str, value: u16) -> Result<()> {
        self.set_raw(key, &value.to_string()).await
    }

    pub async fn get_u32(&self, key: &str, default: u32) -> Result<u32> {
        Ok(self
            .get_raw(key)
            .await?
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(default))
    }

    pub async fn set_u32(&self, key: &str, value: u32) -> Result<()> {
        self.set_raw(key, &value.to_string()).await
    }

    pub async fn get_csv(&self, key: &str, default: &[String]) -> Result<Vec<String>> {
        Ok(self
            .get_raw(key)
            .await?
            .map(|v| parse_csv(&v))
            .unwrap_or_else(|| default.to_vec()))
    }

    pub async fn set_csv(&self, key: &str, values: &[String]) -> Result<()> {
        self.set_raw(key, &values.join(",")).await
    }
}

fn clean_string(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn parse_csv(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn parse_bool(value: &str) -> bool {
    matches!(value, "1" | "true" | "TRUE" | "yes" | "YES")
}
