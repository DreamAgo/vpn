//! 飞书审批“关联外部选项”协议 DTO。
//!
//! 此协议由飞书定义，不使用易链的 [`crate::ApiResponse`] 信封。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ExternalOptionsRequest {
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub employee_id: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub linkage_params: BTreeMap<String, Value>,
    #[serde(default)]
    pub page_token: Option<String>,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub locale: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExternalOptionsResponse {
    pub code: i32,
    pub msg: String,
    pub data: Option<ExternalOptionsData>,
}

impl ExternalOptionsResponse {
    pub fn success(result: ExternalOptionsResult) -> Self {
        Self {
            code: 0,
            msg: "success!".to_string(),
            data: Some(ExternalOptionsData { result }),
        }
    }

    pub fn error(code: i32, msg: impl Into<String>) -> Self {
        Self {
            code,
            msg: msg.into(),
            data: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExternalOptionsData {
    pub result: ExternalOptionsResult,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExternalOptionsResult {
    pub options: Vec<ExternalOption>,
    pub i18n_resources: Vec<ExternalOptionI18nResource>,
    pub has_more: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_page_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExternalOption {
    pub id: String,
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_default: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExternalOptionI18nResource {
    pub locale: String,
    pub is_default: bool,
    pub texts: BTreeMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn serializes_feishu_camel_case_contract() {
        let response = ExternalOptionsResponse::success(ExternalOptionsResult {
            options: vec![ExternalOption {
                id: "subnet-1".into(),
                value: "@i18n@subnet_subnet-1".into(),
                is_default: None,
            }],
            i18n_resources: vec![ExternalOptionI18nResource {
                locale: "zh_cn".into(),
                is_default: true,
                texts: BTreeMap::from([(
                    "@i18n@subnet_subnet-1".into(),
                    "办公网（10.0.0.0/8）".into(),
                )]),
            }],
            has_more: false,
            next_page_token: None,
        });

        let value = serde_json::to_value(response).unwrap();
        assert_eq!(value["msg"], json!("success!"));
        assert_eq!(value["data"]["result"]["hasMore"], json!(false));
        assert!(value["data"]["result"].get("i18nResources").is_some());
        assert!(value["data"]["result"]["options"][0]
            .get("isDefault")
            .is_none());
        assert!(value["data"]["result"].get("nextPageToken").is_none());
    }

    #[test]
    fn request_accepts_missing_optional_fields() {
        let request: ExternalOptionsRequest = serde_json::from_value(json!({})).unwrap();
        assert!(request.token.is_none());
        assert!(request.linkage_params.is_empty());
    }

    #[test]
    fn error_response_keeps_null_data_field() {
        let value =
            serde_json::to_value(ExternalOptionsResponse::error(40100, "unauthorized")).unwrap();
        assert_eq!(value["data"], Value::Null);
    }
}
