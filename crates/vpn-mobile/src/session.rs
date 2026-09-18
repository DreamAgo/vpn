//! Pure control-plane decisions; Android owns serialized HTTPS requests.
use crate::Result;
use serde_json::Value;
pub fn unwrap_response(body: &str) -> Result<Value> {
    let value: Value = serde_json::from_str(body).map_err(|_| "Invalid API JSON")?;
    let code = value
        .get("code")
        .and_then(Value::as_i64)
        .ok_or("Missing API code")?;
    if code != 0 {
        return Err(format!("API error {code}"));
    }
    Ok(value.get("data").cloned().unwrap_or(Value::Null))
}
pub fn session_invalid(code: i32) -> bool {
    matches!(code, 1002 | 1004 | 1007 | 3002)
}
