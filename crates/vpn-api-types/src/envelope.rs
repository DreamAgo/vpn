//! API 响应信封：统一所有 HTTP 响应格式。

use serde::{Deserialize, Serialize};

/// 统一 API 响应信封。
///
/// # 成功示例
///
/// ```json
/// {
///   "code": 0,
///   "message": "success",
///   "data": { "id": "..." },
///   "timestamp": 1747000000000,
///   "request_id": "01HXG..."
/// }
/// ```
///
/// # 错误示例
///
/// ```json
/// {
///   "code": 1001,
///   "message": "用户名或密码错误",
///   "data": null,
///   "timestamp": 1747000000000,
///   "request_id": "01HXG..."
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApiResponse<T> {
    /// 业务码：0 表示成功；非 0 见 [`crate::error_codes`]
    pub code: i32,
    /// 人类可读消息（中文）
    pub message: String,
    /// 响应数据，错误时为 `None`
    pub data: Option<T>,
    /// 响应时间戳（unix milliseconds）
    pub timestamp: i64,
    /// 请求 ID（middleware 注入，便于追踪日志）
    pub request_id: String,
}

impl<T> ApiResponse<T> {
    /// 构造成功响应。
    pub fn success(data: T, request_id: String, timestamp_ms: i64) -> Self {
        Self {
            code: 0,
            message: "success".to_string(),
            data: Some(data),
            timestamp: timestamp_ms,
            request_id,
        }
    }

    /// 构造业务错误响应（HTTP 状态码由调用方决定）。
    pub fn error(
        code: i32,
        message: String,
        request_id: String,
        timestamp_ms: i64,
    ) -> ApiResponse<T> {
        ApiResponse {
            code,
            message,
            data: None,
            timestamp: timestamp_ms,
            request_id,
        }
    }
}

/// 分页响应数据结构。包裹在 `ApiResponse<Page<T>>` 中使用。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub total: u64,
    pub page: u32,
    pub page_size: u32,
}

impl<T> Page<T> {
    pub fn new(items: Vec<T>, total: u64, page: u32, page_size: u32) -> Self {
        Self {
            items,
            total,
            page,
            page_size,
        }
    }

    /// 空分页（搜索无结果）。
    pub fn empty(page: u32, page_size: u32) -> Self {
        Self {
            items: Vec::new(),
            total: 0,
            page,
            page_size,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn success_response_serializes_correctly() {
        let resp: ApiResponse<&str> = ApiResponse::success("hello", "req-1".to_string(), 100);
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains(r#""code":0"#));
        assert!(json.contains(r#""message":"success""#));
        assert!(json.contains(r#""data":"hello""#));
        assert!(json.contains(r#""request_id":"req-1""#));
    }

    #[test]
    fn error_response_has_null_data() {
        let resp: ApiResponse<String> = ApiResponse::error(
            1001,
            "用户名或密码错误".to_string(),
            "req-2".to_string(),
            200,
        );
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains(r#""code":1001"#));
        assert!(json.contains(r#""data":null"#));
    }

    #[test]
    fn page_struct_serializes_correctly() {
        let page: Page<i32> = Page::new(vec![1, 2, 3], 234, 1, 20);
        let json = serde_json::to_string(&page).unwrap();
        assert!(json.contains(r#""items":[1,2,3]"#));
        assert!(json.contains(r#""total":234"#));
        assert!(json.contains(r#""page":1"#));
        assert!(json.contains(r#""page_size":20"#));
    }

    #[test]
    fn empty_page_has_empty_items() {
        let page: Page<i32> = Page::empty(1, 20);
        assert_eq!(page.items.len(), 0);
        assert_eq!(page.total, 0);
    }

    #[test]
    fn api_response_roundtrips() {
        let original: ApiResponse<i32> = ApiResponse::success(42, "req-3".to_string(), 300);
        let json = serde_json::to_string(&original).unwrap();
        let parsed: ApiResponse<i32> = serde_json::from_str(&json).unwrap();
        assert_eq!(original, parsed);
    }
}
