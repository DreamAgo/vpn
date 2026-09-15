//! Updates belong exclusively to the currently configured VPN server.
use serde::Serialize;
use std::time::Duration;
use tauri::Manager;
use tauri_plugin_updater::UpdaterExt;

fn endpoint(server: Option<&str>) -> Result<url::Url, String> {
    let server = server
        .filter(|s| !s.trim().is_empty())
        .ok_or("请先配置并登录服务端，再检查更新")?;
    let mut url = url::Url::parse(server.trim()).map_err(|_| "服务端地址无效")?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("更新服务端必须是有效的 HTTPS 地址，不能包含凭证、查询参数或片段".into());
    }
    url.set_path(&format!(
        "{}/updates/latest.json",
        url.path().trim_end_matches('/')
    ));
    Ok(url)
}

#[tauri::command]
pub fn update_source() -> Result<String, String> {
    let server = crate::commands::repo()?
        .server_url()
        .map_err(|e| e.to_string())?;
    Ok(endpoint(server.as_deref())?.to_string())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateMetadata {
    rid: tauri::ResourceId,
    current_version: String,
    version: String,
    body: Option<String>,
    raw_json: serde_json::Value,
    source: String,
}

#[tauri::command]
pub async fn check_server_update(
    webview: tauri::Webview,
) -> Result<Option<UpdateMetadata>, String> {
    let source = update_source()?;
    let updater = webview
        .updater_builder()
        .endpoints(vec![url::Url::parse(&source).map_err(|e| e.to_string())?])
        .map_err(|e| e.to_string())?
        .timeout(Duration::from_secs(10))
        // Require an actual manifest even when no newer version is available.
        // Plugin's None for HTTP 204 must not be reported as "already latest".
        .version_comparator(|_, _| true)
        .build()
        .map_err(|e| e.to_string())?;
    let update = updater
        .check()
        .await
        .map_err(|e| format!("无法读取当前服务端的更新清单（{source}）：{e}"))?
        .ok_or("当前服务端未提供更新清单，无法判断是否为最新版本")?;
    if update_source()? != source {
        return Err("服务端已切换，请重新检查更新".into());
    }
    let remote = semver::Version::parse(&update.version).map_err(|_| "服务端更新版本号无效")?;
    let current = semver::Version::parse(&update.current_version).map_err(|_| "本机版本号无效")?;
    if remote <= current {
        return Ok(None);
    }
    Ok(Some(UpdateMetadata {
        current_version: update.current_version.clone(),
        version: update.version.clone(),
        body: update.body.clone(),
        raw_json: update.raw_json.clone(),
        source,
        rid: webview.resources_table().add(update),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_is_only_the_configured_server() {
        assert_eq!(
            endpoint(Some("https://vpn.example:8443/"))
                .unwrap()
                .as_str(),
            "https://vpn.example:8443/updates/latest.json"
        );
        assert_eq!(
            endpoint(Some("https://other.example/vpn/"))
                .unwrap()
                .as_str(),
            "https://other.example/vpn/updates/latest.json"
        );
        for input in [
            None,
            Some(""),
            Some("http://vpn.example"),
            Some("https://user:pass@vpn.example"),
            Some("https://vpn.example?q=1"),
            Some("https://vpn.example/#test"),
        ] {
            assert!(endpoint(input).is_err(), "{input:?}");
        }
    }
}
