//! 飞书审批外部选项服务：认证、provider 分发、搜索和带签名游标分页。

use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use vpn_api_types::external_options::{
    ExternalOption, ExternalOptionI18nResource, ExternalOptionsRequest, ExternalOptionsResult,
};
use vpn_core::AppError;

use super::SubnetService;

type HmacSha256 = Hmac<Sha256>;
const PAGE_SIZE: usize = 50;
const CURSOR_VERSION: u8 = 1;
const TOKEN_CHECK_MESSAGE: &[u8] = b"yilian-feishu-approval-options-token";
const MAX_TOKEN_LENGTH: usize = 512;
const MAX_QUERY_LENGTH: usize = 256;
const MAX_CURSOR_LENGTH: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalOptionItem {
    pub id: String,
    pub label: String,
    pub is_default: bool,
}

#[async_trait]
pub trait ExternalOptionProvider: Send + Sync {
    async fn items(&self) -> Result<Vec<ExternalOptionItem>, AppError>;
}

pub struct SubnetExternalOptionProvider {
    subnets: Arc<SubnetService>,
}

impl SubnetExternalOptionProvider {
    pub fn new(subnets: Arc<SubnetService>) -> Self {
        Self { subnets }
    }
}

#[async_trait]
impl ExternalOptionProvider for SubnetExternalOptionProvider {
    async fn items(&self) -> Result<Vec<ExternalOptionItem>, AppError> {
        Ok(self
            .subnets
            .repo
            .list()
            .await?
            .into_iter()
            .map(|subnet| ExternalOptionItem {
                id: subnet.id,
                label: format!("{}（{}）", subnet.name, subnet.cidr),
                is_default: false,
            })
            .collect())
    }
}

pub struct ExternalOptionsService {
    token: Option<Arc<str>>,
    providers: HashMap<String, Arc<dyn ExternalOptionProvider>>,
}

impl ExternalOptionsService {
    pub fn new(token: Option<String>) -> Self {
        Self {
            token: token
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .map(Arc::from),
            providers: HashMap::new(),
        }
    }

    pub fn register(
        &mut self,
        source: impl Into<String>,
        provider: Arc<dyn ExternalOptionProvider>,
    ) -> Result<(), ExternalOptionsRegistrationError> {
        let source = source.into();
        if self.providers.contains_key(&source) {
            return Err(ExternalOptionsRegistrationError::DuplicateSource(source));
        }
        self.providers.insert(source, provider);
        Ok(())
    }

    pub async fn query(
        &self,
        source: &str,
        request: &ExternalOptionsRequest,
    ) -> Result<ExternalOptionsResult, ExternalOptionsError> {
        let token = self
            .token
            .as_deref()
            .ok_or(ExternalOptionsError::NotConfigured)?;
        let supplied_token = request
            .token
            .as_deref()
            .ok_or(ExternalOptionsError::Unauthorized)?;
        if supplied_token.len() > MAX_TOKEN_LENGTH {
            return Err(ExternalOptionsError::Unauthorized);
        }
        if !tokens_equal(token.as_bytes(), supplied_token.as_bytes()) {
            return Err(ExternalOptionsError::Unauthorized);
        }

        let provider = self
            .providers
            .get(source)
            .ok_or(ExternalOptionsError::UnknownSource)?;
        let normalized_query = request
            .query
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .to_lowercase();
        if normalized_query.len() > MAX_QUERY_LENGTH {
            return Err(ExternalOptionsError::InvalidRequest);
        }
        let offset = match request.page_token.as_deref() {
            Some(cursor) if !cursor.trim().is_empty() => {
                if cursor.len() > MAX_CURSOR_LENGTH {
                    return Err(ExternalOptionsError::InvalidCursor);
                }
                decode_cursor(cursor, token.as_bytes(), source, &normalized_query)?
            }
            _ => 0,
        };

        let mut items = provider
            .items()
            .await
            .map_err(ExternalOptionsError::Backend)?;
        if !normalized_query.is_empty() {
            items.retain(|item| item.label.to_lowercase().contains(&normalized_query));
        }
        items.sort_by(|left, right| {
            left.label
                .cmp(&right.label)
                .then_with(|| left.id.cmp(&right.id))
        });

        if offset > items.len() {
            return Err(ExternalOptionsError::InvalidCursor);
        }
        let end = offset.saturating_add(PAGE_SIZE).min(items.len());
        let page = &items[offset..end];
        let has_more = end < items.len();
        let next_page_token = has_more
            .then(|| encode_cursor(token.as_bytes(), source, &normalized_query, end))
            .transpose()?;

        let mut texts = std::collections::BTreeMap::new();
        let options = page
            .iter()
            .map(|item| {
                let value = format!("@i18n@{source}_{}", item.id);
                texts.insert(value.clone(), item.label.clone());
                ExternalOption {
                    id: item.id.clone(),
                    value,
                    is_default: item.is_default.then_some(true),
                }
            })
            .collect();

        Ok(ExternalOptionsResult {
            options,
            i18n_resources: vec![ExternalOptionI18nResource {
                locale: "zh_cn".to_string(),
                is_default: true,
                texts,
            }],
            has_more,
            next_page_token,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ExternalOptionsError {
    #[error("飞书审批外部选项未配置")]
    NotConfigured,
    #[error("请求来源校验失败")]
    Unauthorized,
    #[error("未知的外部选项数据源")]
    UnknownSource,
    #[error("分页标记无效")]
    InvalidCursor,
    #[error("请求参数无效")]
    InvalidRequest,
    #[error("读取外部选项超时")]
    Timeout,
    #[error("读取外部选项失败")]
    Backend(#[source] AppError),
}

#[derive(Debug, thiserror::Error)]
pub enum ExternalOptionsRegistrationError {
    #[error("外部选项数据源重复注册: {0}")]
    DuplicateSource(String),
}

#[derive(Serialize, Deserialize)]
struct CursorPayload {
    version: u8,
    source: String,
    query_hash: String,
    offset: usize,
}

fn query_hash(query: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(query.as_bytes()))
}

fn encode_cursor(
    secret: &[u8],
    source: &str,
    query: &str,
    offset: usize,
) -> Result<String, ExternalOptionsError> {
    let payload = serde_json::to_vec(&CursorPayload {
        version: CURSOR_VERSION,
        source: source.to_string(),
        query_hash: query_hash(query),
        offset,
    })
    .map_err(|_| ExternalOptionsError::InvalidCursor)?;
    let signature = sign(secret, &payload)?;
    Ok(format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(payload),
        URL_SAFE_NO_PAD.encode(signature)
    ))
}

fn decode_cursor(
    cursor: &str,
    secret: &[u8],
    source: &str,
    query: &str,
) -> Result<usize, ExternalOptionsError> {
    let (payload, signature) = cursor
        .split_once('.')
        .ok_or(ExternalOptionsError::InvalidCursor)?;
    let payload = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| ExternalOptionsError::InvalidCursor)?;
    let signature = URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| ExternalOptionsError::InvalidCursor)?;
    let mut verifier =
        HmacSha256::new_from_slice(secret).map_err(|_| ExternalOptionsError::InvalidCursor)?;
    verifier.update(&payload);
    verifier
        .verify_slice(&signature)
        .map_err(|_| ExternalOptionsError::InvalidCursor)?;
    let payload: CursorPayload =
        serde_json::from_slice(&payload).map_err(|_| ExternalOptionsError::InvalidCursor)?;
    if payload.version != CURSOR_VERSION
        || payload.source != source
        || payload.query_hash != query_hash(query)
    {
        return Err(ExternalOptionsError::InvalidCursor);
    }
    Ok(payload.offset)
}

fn sign(secret: &[u8], payload: &[u8]) -> Result<Vec<u8>, ExternalOptionsError> {
    let mut mac =
        HmacSha256::new_from_slice(secret).map_err(|_| ExternalOptionsError::InvalidCursor)?;
    mac.update(payload);
    Ok(mac.finalize().into_bytes().to_vec())
}

fn tokens_equal(expected: &[u8], supplied: &[u8]) -> bool {
    let Ok(mut verifier) = HmacSha256::new_from_slice(expected) else {
        return false;
    };
    verifier.update(TOKEN_CHECK_MESSAGE);

    let Ok(mut supplied_mac) = HmacSha256::new_from_slice(supplied) else {
        return false;
    };
    supplied_mac.update(TOKEN_CHECK_MESSAGE);
    let supplied_tag = supplied_mac.finalize().into_bytes();
    verifier.verify_slice(&supplied_tag).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StaticProvider(Vec<ExternalOptionItem>);

    #[async_trait]
    impl ExternalOptionProvider for StaticProvider {
        async fn items(&self) -> Result<Vec<ExternalOptionItem>, AppError> {
            Ok(self.0.clone())
        }
    }

    fn service(count: usize) -> ExternalOptionsService {
        let mut service = ExternalOptionsService::new(Some("secret".into()));
        service
            .register(
                "items",
                Arc::new(StaticProvider(
                    (0..count)
                        .map(|index| ExternalOptionItem {
                            id: format!("id-{index:03}"),
                            label: format!("选项 {index:03}"),
                            is_default: false,
                        })
                        .collect(),
                )),
            )
            .unwrap();
        service
    }

    fn request() -> ExternalOptionsRequest {
        ExternalOptionsRequest {
            token: Some("secret".into()),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn paginates_and_rejects_cursor_for_other_query() {
        let service = service(55);
        let first = service.query("items", &request()).await.unwrap();
        assert_eq!(first.options.len(), PAGE_SIZE);
        assert!(first.has_more);

        let mut second_request = request();
        second_request.page_token = first.next_page_token;
        let second = service.query("items", &second_request).await.unwrap();
        assert_eq!(second.options.len(), 5);
        assert!(!second.has_more);

        second_request.query = Some("不同查询".into());
        assert!(matches!(
            service.query("items", &second_request).await,
            Err(ExternalOptionsError::InvalidCursor)
        ));
    }

    #[tokio::test]
    async fn rejects_tampered_cursor_and_bad_token() {
        let service = service(55);
        let first = service.query("items", &request()).await.unwrap();
        let mut next = request();
        next.page_token = first.next_page_token.map(|token| format!("{token}x"));
        assert!(matches!(
            service.query("items", &next).await,
            Err(ExternalOptionsError::InvalidCursor)
        ));

        next.page_token = None;
        next.token = Some("wrong".into());
        assert!(matches!(
            service.query("items", &next).await,
            Err(ExternalOptionsError::Unauthorized)
        ));
    }

    #[tokio::test]
    async fn rejects_oversized_query_before_loading_provider() {
        let service = service(1);
        let mut oversized = request();
        oversized.query = Some("x".repeat(MAX_QUERY_LENGTH + 1));
        assert!(matches!(
            service.query("items", &oversized).await,
            Err(ExternalOptionsError::InvalidRequest)
        ));
    }

    #[test]
    fn rejects_duplicate_provider_registration() {
        let mut service = ExternalOptionsService::new(Some("secret".into()));
        service
            .register("items", Arc::new(StaticProvider(Vec::new())))
            .unwrap();
        assert!(matches!(
            service.register("items", Arc::new(StaticProvider(Vec::new()))),
            Err(ExternalOptionsRegistrationError::DuplicateSource(source)) if source == "items"
        ));
    }
}
