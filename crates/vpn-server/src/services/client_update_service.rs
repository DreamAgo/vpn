//! Mirror signed client installers from the fixed GitHub repository before publishing a local manifest.
use base64::{engine::general_purpose::STANDARD, Engine};
use futures::{stream, StreamExt, TryStreamExt};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{Mutex, OwnedMutexGuard},
};

const REPO: &str = "DreamAgo/vpn";
const LIMIT: usize = 1024 * 1024;
const INTERVAL_MS: i64 = 60 * 60 * 1000;
const MAX_PACKAGE: u64 = 1024 * 1024 * 1024;
const MAX_RELEASE: u64 = 4 * MAX_PACKAGE;
type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlatformUpdate {
    pub url: String,
    pub signature: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LocalAsset {
    pub name: String,
    pub url: String,
    pub size: u64,
    pub digest: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pub_date: Option<String>,
    pub platforms: BTreeMap<String, PlatformUpdate>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub downloads: Vec<LocalAsset>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct SyncRecord {
    pub auto_sync: bool,
    #[serde(default)]
    pub public_base_url: String,
    #[serde(default)]
    pub proxy_url: String,
    #[serde(default)]
    pub minimum_client_version: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub github_token: String,
    pub last_checked_at: Option<i64>,
    pub last_synced_at: Option<i64>,
    pub last_error: Option<String>,
}
#[derive(Serialize)]
pub struct UpdateStatus {
    #[serde(flatten)]
    pub record: SyncRecord,
    pub github_token_set: bool,
    pub repository: &'static str,
    pub syncing: bool,
    pub next_check_at: Option<i64>,
    pub manifest: Option<Manifest>,
}
#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}
#[derive(Clone, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    size: u64,
    #[serde(default)]
    digest: Option<String>,
}

pub struct ClientUpdateService {
    root: PathBuf,
    record: Mutex<SyncRecord>,
    gate: Arc<Mutex<()>>,
}
impl ClientUpdateService {
    pub fn new(root: PathBuf) -> Self {
        let record = match std::fs::read(root.join("client-update-sync.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|_| SyncRecord {
                last_error: Some("同步设置文件损坏，请重新保存自动同步设置".into()),
                ..Default::default()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => SyncRecord::default(),
            Err(_) => SyncRecord {
                last_error: Some("无法读取同步设置文件".into()),
                ..Default::default()
            },
        };
        Self {
            root,
            record: Mutex::new(record),
            gate: Arc::new(Mutex::new(())),
        }
    }
    fn manifest_path(&self) -> PathBuf {
        self.root.join("updates/latest.json")
    }
    async fn current(&self) -> Result<Option<Manifest>> {
        match tokio::fs::read(self.manifest_path()).await {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|_| "当前更新清单损坏".into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err("无法读取当前更新清单".into()),
        }
    }
    pub async fn status(&self) -> UpdateStatus {
        let mut record = self.record.lock().await.clone();
        let manifest = match self.current().await {
            Ok(m) => m,
            Err(e) => {
                record.last_error = Some(e);
                None
            }
        };
        let next_check_at = record.auto_sync.then(|| {
            record
                .last_checked_at
                .map(|t| t + INTERVAL_MS)
                .unwrap_or_else(|| chrono::Utc::now().timestamp_millis() + 60_000)
        });
        let github_token_set = !record.github_token.is_empty();
        record.github_token.clear();
        UpdateStatus {
            github_token_set,
            record,
            repository: REPO,
            syncing: self.gate.try_lock().is_err(),
            next_check_at,
            manifest,
        }
    }
    pub async fn configure(
        &self,
        enabled: bool,
        base_url: &str,
        proxy_url: Option<&str>,
        github_token: Option<&str>,
        minimum_client_version: Option<&str>,
    ) -> Result<()> {
        let minimum_client_version = minimum_client_version
            .map(|value| {
                let value = value.trim();
                if value.is_empty() {
                    Ok(String::new())
                } else {
                    version(value).map(|v| v.to_string())
                }
            })
            .transpose()?;
        let github_token = github_token.map(normalize_token).transpose()?;
        let base_url = normalize_base(base_url)?;
        let proxy_url = proxy_url.map(normalize_proxy).transpose()?;
        let _guard = self
            .gate
            .try_lock()
            .map_err(|_| "正在同步，请稍后再保存设置")?;
        let mut record = self.record.lock().await;
        let mut next = record.clone();
        if enabled && !next.auto_sync {
            next.last_checked_at = None;
        }
        next.auto_sync = enabled;
        next.public_base_url = base_url;
        if let Some(minimum) = minimum_client_version {
            next.minimum_client_version = minimum;
        }
        if let Some(token) = github_token {
            next.github_token = token;
        }
        if let Some(proxy_url) = proxy_url {
            next.proxy_url = proxy_url;
        }
        atomic_json(&self.root.join("client-update-sync.json"), &next).await?;
        *record = next;
        Ok(())
    }
    pub async fn enforce_client_version(&self, reported: Option<&str>) -> Result<()> {
        check_client_version(&self.record.lock().await.minimum_client_version, reported)
    }

    pub fn start(service: &Arc<Self>) {
        let weak = Arc::downgrade(service);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                let Some(service) = weak.upgrade() else { break };
                let record = service.record.lock().await.clone();
                if record.auto_sync
                    && record.last_checked_at.is_none_or(|last| {
                        chrono::Utc::now().timestamp_millis() - last >= INTERVAL_MS
                    })
                {
                    if let Err(error) = service.sync().await {
                        tracing::warn!(%error, "客户端版本自动同步失败");
                    }
                }
            }
        });
    }
    pub fn queue(self: &Arc<Self>) -> Result<()> {
        let guard = self
            .gate
            .clone()
            .try_lock_owned()
            .map_err(|_| "正在同步，请稍后再试")?;
        let service = self.clone();
        tokio::spawn(async move {
            if let Err(error) = service.sync_locked(guard).await {
                tracing::warn!(%error, "客户端版本同步失败");
            }
        });
        Ok(())
    }
    pub async fn sync(&self) -> Result<()> {
        let guard = self
            .gate
            .clone()
            .try_lock_owned()
            .map_err(|_| "正在同步，请稍后再试")?;
        self.sync_locked(guard).await
    }
    async fn sync_locked(&self, _guard: OwnedMutexGuard<()>) -> Result<()> {
        let stage = self
            .root
            .join(format!(".client-update-staging-{}", uuid::Uuid::now_v7()));
        let base = self.record.lock().await.public_base_url.clone();
        let result = tokio::time::timeout(
            Duration::from_secs(1800),
            self.fetch_and_publish(&stage, &base),
        )
        .await
        .unwrap_or_else(|_| Err("安装包同步超过 30 分钟，请稍后重试".into()));
        let _ = tokio::fs::remove_dir_all(&stage).await;
        let mut record = self.record.lock().await;
        let now = chrono::Utc::now().timestamp_millis();
        record.last_checked_at = Some(now);
        record.last_error = result.as_ref().err().cloned();
        if result.is_ok() {
            record.last_synced_at = Some(now);
        }
        atomic_json(&self.root.join("client-update-sync.json"), &*record).await?;
        result
    }
    async fn fetch_and_publish(&self, stage: &Path, base: &str) -> Result<()> {
        let base = normalize_base(base)?;
        let record = self.record.lock().await.clone();
        let client = github_client(&record.proxy_url)?;
        // Token is attached only to the fixed GitHub API request; never follow its redirects.
        let api_client = github_client_for(&record.proxy_url, true)?;
        let mut request = api_client.get(format!(
            "https://api.github.com/repos/{REPO}/releases/latest"
        ));
        if !record.github_token.is_empty() {
            request = request.bearer_auth(&record.github_token);
        }
        let bytes = download_request(request).await?;
        let release: Release =
            serde_json::from_slice(&bytes).map_err(|_| "GitHub 发布信息格式错误".to_string())?;
        if release.draft || release.prerelease {
            return Err("仅同步正式发布版本".into());
        }
        let asset = release
            .assets
            .iter()
            .find(|a| a.name == "latest.json")
            .ok_or("最新发布没有 latest.json，请检查 GitHub 发布流程")?;
        let prefix = format!(
            "https://github.com/{REPO}/releases/download/{}/",
            release.tag_name
        );
        if asset.browser_download_url != format!("{prefix}latest.json") || asset.size > LIMIT as u64
        {
            return Err("GitHub 更新清单地址或大小不符合要求".into());
        }
        let bytes = download(&client, &asset.browser_download_url).await?;
        let mut manifest: Manifest =
            serde_json::from_slice(&bytes).map_err(|_| "更新清单 JSON 格式错误".to_string())?;
        manifest
            .platforms
            .retain(|platform, _| !platform.starts_with("linux-"));
        manifest
            .downloads
            .retain(|asset| !asset.name.starts_with("vpn-gui-linux-"));
        validate(&manifest, &release)?;
        let packages = package_assets(&release)?;
        if let Some(current) = self.current().await? {
            if version(&manifest.version)? < version(&current.version)? {
                return Err("GitHub 最新版本低于已发布版本，已阻止降级".into());
            }
            if current.version == manifest.version
                && self
                    .local_release_valid(&current, &manifest, &packages, &base)
                    .await
            {
                let mut refreshed = current;
                refreshed.notes = manifest.notes;
                refreshed.pub_date = manifest.pub_date;
                return atomic_json(&self.manifest_path(), &refreshed).await;
            }
        }
        tokio::fs::create_dir_all(stage)
            .await
            .map_err(|_| "无法创建下载目录，请检查磁盘空间和权限")?;
        let downloads: Vec<Asset> = packages.iter().map(|a| (*a).clone()).collect();
        let download_root = stage.to_path_buf();
        stream::iter(downloads)
            .map(move |asset| {
                let client = client.clone();
                let path = download_root.join(&asset.name);
                async move { download_package(&client, &asset, &path).await }
            })
            .buffer_unordered(3)
            .try_collect::<Vec<_>>()
            .await?;
        self.publish_downloaded(stage, manifest, &release, &base)
            .await
    }
    async fn publish_downloaded(
        &self,
        stage: &Path,
        mut manifest: Manifest,
        release: &Release,
        base: &str,
    ) -> Result<()> {
        validate(&manifest, release)?;
        let packages = package_assets(release)?;
        for asset in &packages {
            verify_file(&stage.join(&asset.name), asset).await?;
        }
        // The exact published sidecar must match the signature supplied to clients.
        for entry in manifest.platforms.values() {
            let name = entry.url.rsplit('/').next().ok_or("无效更新包地址")?;
            let signature = tokio::fs::read_to_string(stage.join(format!("{name}.sig")))
                .await
                .map_err(|_| "无法读取更新签名")?;
            if signature.trim() != entry.signature.trim() {
                return Err("清单签名与下载的签名文件不一致".into());
            }
        }
        let id = uuid::Uuid::now_v7().to_string();
        manifest.downloads = packages
            .iter()
            .map(|a| LocalAsset {
                name: a.name.clone(),
                size: a.size,
                digest: a.digest.clone().unwrap_or_default(),
                url: format!("{base}/updates/releases/{id}/{}", a.name),
            })
            .collect();
        for entry in manifest.platforms.values_mut() {
            let name = entry.url.rsplit('/').next().ok_or("无效更新包地址")?;
            entry.url = format!("{base}/updates/releases/{id}/{name}");
        }
        let releases = self.root.join("updates/releases");
        tokio::fs::create_dir_all(&releases)
            .await
            .map_err(|_| "无法创建发布目录")?;
        let published = releases.join(&id);
        tokio::fs::rename(stage, &published)
            .await
            .map_err(|_| "无法发布安装包目录")?;
        if let Err(error) = atomic_json(&self.manifest_path(), &manifest).await {
            let _ = tokio::fs::remove_dir_all(published).await;
            return Err(error);
        }
        Ok(())
    }
    async fn local_release_valid(
        &self,
        current: &Manifest,
        source: &Manifest,
        assets: &[&Asset],
        base: &str,
    ) -> bool {
        if current.downloads.len() != assets.len()
            || current.platforms.len() != source.platforms.len()
        {
            return false;
        }
        for (target, remote) in &source.platforms {
            let Some(local) = current.platforms.get(target) else {
                return false;
            };
            let name = remote.url.rsplit('/').next().unwrap_or_default();
            if local.signature != remote.signature
                || !current
                    .downloads
                    .iter()
                    .any(|a| a.name == name && a.url == local.url)
            {
                return false;
            }
        }
        for asset in assets {
            let Some(local) = current.downloads.iter().find(|a| a.name == asset.name) else {
                return false;
            };
            let Some(path) = local_path(&self.root, base, &local.url, &asset.name) else {
                return false;
            };
            if verify_file(&path, asset).await.is_err() {
                return false;
            }
        }
        true
    }
}
fn normalize_base(value: &str) -> Result<String> {
    let url = reqwest::Url::parse(value).map_err(|_| "请先设置客户端可访问的服务端地址")?;
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if (url.scheme() != "https" && !(local && url.scheme() == "http"))
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err("服务端地址须为 HTTPS 域名及端口，不包含路径、账号或查询参数".into());
    }
    Ok(url.origin().ascii_serialization())
}
fn package_assets(release: &Release) -> Result<Vec<&Asset>> {
    let assets: Vec<_> = release
        .assets
        .iter()
        .filter(|a| {
            (a.name.starts_with("vpn-gui-") && !a.name.starts_with("vpn-gui-linux-"))
                || (a.name.starts_with("vpn-android-") && a.name.ends_with(".apk"))
        })
        .collect();
    let android_name = format!(
        "vpn-android-universal-{}.apk",
        release.tag_name.trim_start_matches('v')
    );
    let mut total = 0u64;
    let mut names = std::collections::HashSet::new();
    for a in &assets {
        if (a.name.starts_with("vpn-android-")
            && (a.name != android_name || a.size > 256 * 1024 * 1024))
            || !names.insert(&a.name)
            || !a
                .name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-._".contains(&c))
            || a.name.contains("..")
            || a.size == 0
            || a.size > MAX_PACKAGE
            || a.browser_download_url
                != format!(
                    "https://github.com/{REPO}/releases/download/{}/{}",
                    release.tag_name, a.name
                )
        {
            return Err("发布中的安装包名称、地址或大小不符合要求".into());
        }
        let digest = a
            .digest
            .as_deref()
            .unwrap_or_default()
            .strip_prefix("sha256:")
            .ok_or("GitHub 安装包缺少 SHA-256 摘要")?;
        if digest.len() != 64 || !digest.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err("GitHub SHA-256 摘要无效".into());
        }
        total = total.checked_add(a.size).ok_or("发布大小超限")?;
        if total > MAX_RELEASE {
            return Err("安装包总大小超过 4 GiB 限制".into());
        }
    }
    Ok(assets)
}
fn local_path(root: &Path, base: &str, url: &str, name: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix(&format!("{base}/updates/releases/"))?;
    let (id, file) = rest.split_once('/')?;
    uuid::Uuid::parse_str(id).ok()?;
    if file != name || file.contains('/') || file.contains("..") {
        return None;
    }
    Some(root.join("updates/releases").join(id).join(file))
}
async fn verify_file(path: &Path, asset: &Asset) -> Result<()> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|_| "本地安装包不存在")?;
    if file
        .metadata()
        .await
        .map_err(|_| "无法读取本地安装包")?
        .len()
        != asset.size
    {
        return Err("安装包大小不匹配".into());
    }
    let mut hash = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).await.map_err(|_| "读取安装包失败")?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    if asset.digest.as_deref() != Some(format!("sha256:{:x}", hash.finalize()).as_str()) {
        return Err("安装包 SHA-256 校验失败".into());
    }
    Ok(())
}
async fn download_package(client: &reqwest::Client, asset: &Asset, path: &Path) -> Result<()> {
    let mut response = client
        .get(&asset.browser_download_url)
        .send()
        .await
        .map_err(|_| format!("下载 {} 失败", asset.name))?;
    if !response.status().is_success() {
        return Err(format!(
            "下载 {} 返回 HTTP {}",
            asset.name,
            response.status()
        ));
    }
    let mut file = tokio::fs::File::create(path)
        .await
        .map_err(|_| "无法保存安装包，请检查磁盘空间和权限")?;
    let mut size = 0u64;
    while let Some(chunk) = response.chunk().await.map_err(|_| "安装包下载中断")? {
        size += chunk.len() as u64;
        if size > asset.size {
            return Err("安装包超过 GitHub 声明的大小".into());
        }
        file.write_all(&chunk)
            .await
            .map_err(|_| "写入安装包失败，请检查剩余磁盘空间")?;
    }
    file.sync_all().await.map_err(|_| "保存安装包失败")?;
    drop(file);
    verify_file(path, asset).await
}
fn normalize_proxy(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(String::new());
    }
    let url = reqwest::Url::parse(value)
        .map_err(|_| "代理地址格式无效，请填写 http://主机:端口 或 https://主机:端口")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !matches!(url.path(), "" | "/")
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("代理只支持 HTTP/HTTPS 地址，不能包含路径、查询参数或片段".into());
    }
    Ok(url.to_string())
}

fn check_client_version(minimum: &str, reported: Option<&str>) -> Result<()> {
    if minimum.is_empty() {
        return Ok(());
    }
    let minimum =
        version(minimum).map_err(|_| "最低客户端版本配置无效，请联系管理员".to_string())?;
    let client = reported
        .and_then(|s| semver::Version::parse(s.trim().strip_prefix('v').unwrap_or(s.trim())).ok());
    match client {
        Some(client) if !client.cmp_precedence(&minimum).is_lt() => Ok(()),
        Some(client) => Err(format!(
            "客户端版本 {client} 低于服务端要求的 {minimum}，请升级客户端后重新连接"
        )),
        None => Err(format!(
            "无法识别客户端版本，服务端要求 {minimum} 或更高版本，请升级客户端后重新连接"
        )),
    }
}

fn normalize_token(value: &str) -> Result<String> {
    let value = value.trim();
    if value.len() > 512
        || !value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_')
    {
        return Err("GitHub Token 格式无效".into());
    }
    Ok(value.to_owned())
}

fn github_client(proxy_url: &str) -> Result<reqwest::Client> {
    github_client_for(proxy_url, false)
}

fn github_client_for(proxy_url: &str, api: bool) -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .user_agent("yilian-client-update-sync")
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(600))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() < 5 && allowed_host(attempt.url()) {
                attempt.follow()
            } else {
                attempt.error("unexpected GitHub redirect")
            }
        }));
    if api {
        builder = builder.redirect(reqwest::redirect::Policy::none());
    }
    let proxy_url = normalize_proxy(proxy_url)?;
    if !proxy_url.is_empty() {
        builder = builder.proxy(reqwest::Proxy::all(&proxy_url).map_err(|_| "代理地址无效")?);
    }
    builder
        .build()
        .map_err(|_| "无法创建 GitHub 客户端".to_string())
}

fn allowed_host(url: &reqwest::Url) -> bool {
    url.scheme() == "https"
        && matches!(
            url.host_str(),
            Some(
                "api.github.com"
                    | "github.com"
                    | "release-assets.githubusercontent.com"
                    | "objects.githubusercontent.com"
            )
        )
}
async fn download(client: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    download_request(client.get(url)).await
}
async fn download_request(request: reqwest::RequestBuilder) -> Result<Vec<u8>> {
    let mut response = request
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|_| "无法连接 GitHub，请检查服务器网络".to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "GitHub 返回 HTTP {}，请稍后重试",
            response.status().as_u16()
        ));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "GitHub 下载中断".to_string())?
    {
        if bytes.len() + chunk.len() > LIMIT {
            return Err("GitHub 响应超过大小限制".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
fn version(value: &str) -> Result<semver::Version> {
    let v = semver::Version::parse(value.strip_prefix('v').unwrap_or(value))
        .map_err(|_| "版本号格式错误".to_string())?;
    if !v.pre.is_empty() {
        return Err("不发布预览版本".into());
    }
    Ok(v)
}
fn validate(manifest: &Manifest, release: &Release) -> Result<()> {
    if version(&manifest.version)? != version(&release.tag_name)? {
        return Err("清单版本与 GitHub 标签不一致".into());
    }
    if let Some(date) = &manifest.pub_date {
        chrono::DateTime::parse_from_rfc3339(date).map_err(|_| "清单发布时间格式错误")?;
    }
    let required = [
        ("windows-x86_64", "windows-amd64-setup", "exe"),
        ("darwin-x86_64", "macos-amd64", "app.tar.gz"),
        ("darwin-aarch64", "macos-arm64", "app.tar.gz"),
    ];
    if manifest.platforms.len() != required.len() {
        return Err("更新清单平台不完整或包含未知平台".into());
    }
    for (target, name, extension) in required {
        let entry = manifest
            .platforms
            .get(target)
            .ok_or_else(|| format!("缺少平台 {target}"))?;
        let filename = format!("vpn-gui-{name}-{}.{extension}", release.tag_name);
        let expected = format!(
            "https://github.com/{REPO}/releases/download/{}/{filename}",
            release.tag_name
        );
        if entry.url != expected {
            return Err(format!("{target} 更新包地址不匹配"));
        }
        for name in [filename.clone(), format!("{filename}.sig")] {
            if !release.assets.iter().any(|a| {
                a.name == name
                    && a.size > 0
                    && a.browser_download_url
                        == format!(
                            "https://github.com/{REPO}/releases/download/{}/{name}",
                            release.tag_name
                        )
            }) {
                return Err(format!("{target} 更新包或签名尚未发布"));
            }
        }
        let sig = STANDARD
            .decode(entry.signature.trim())
            .map_err(|_| format!("{target} 签名编码无效"))?;
        if sig.len() < 80 {
            return Err(format!("{target} 签名无效"));
        }
    }
    Ok(())
}
async fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().ok_or("无效存储路径")?;
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|_| "无法创建版本存储目录")?;
    let tmp = path.with_extension(format!("{}.tmp", uuid::Uuid::now_v7()));
    let bytes = serde_json::to_vec_pretty(value).map_err(|_| "无法序列化版本信息")?;
    let result = async {
        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        if path.file_name().and_then(|name| name.to_str()) == Some("client-update-sync.json") {
            options.mode(0o600);
        }
        let mut file = options.open(&tmp).await?;
        file.write_all(&bytes).await?;
        file.flush().await?;
        drop(file);
        tokio::fs::rename(&tmp, path).await
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&tmp).await;
    }
    result.map_err(|_| "无法保存版本信息，请检查数据目录权限".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn proxy_validation_and_old_settings() {
        assert_eq!(normalize_proxy("  ").unwrap(), "");
        assert_eq!(
            normalize_proxy(" http://proxy.example:7897 ").unwrap(),
            "http://proxy.example:7897/"
        );
        assert!(normalize_proxy("https://user:password@proxy.example:443").is_ok());
        for invalid in [
            "proxy:7897",
            "socks5://proxy:1080",
            "http://proxy/path",
            "http://proxy?q=1",
            "http://proxy/#fragment",
        ] {
            assert!(normalize_proxy(invalid).is_err());
        }
        let record: SyncRecord = serde_json::from_str(r#"{"auto_sync":false}"#).unwrap();
        assert!(record.proxy_url.is_empty());
    }

    #[tokio::test]
    async fn github_requests_use_configured_proxy() {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let proxy = tokio::spawn(async move {
            let mut requests = Vec::new();
            for _ in 0..3 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = [0; 4096];
                let n = socket.read(&mut bytes).await.unwrap();
                requests.push(String::from_utf8_lossy(&bytes[..n]).to_string());
                socket
                    .write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n")
                    .await
                    .unwrap();
            }
            requests
        });
        let client = github_client(&format!("http://{address}")).unwrap();
        for host in [
            "api.github.com",
            "github.com",
            "release-assets.githubusercontent.com",
        ] {
            assert!(client
                .get(format!("https://{host}/test"))
                .timeout(Duration::from_secs(2))
                .send()
                .await
                .is_err());
        }
        let requests = tokio::time::timeout(Duration::from_secs(3), proxy)
            .await
            .unwrap()
            .unwrap();
        for (request, host) in requests.iter().zip([
            "api.github.com",
            "github.com",
            "release-assets.githubusercontent.com",
        ]) {
            assert!(request.starts_with(&format!("CONNECT {host}:443 HTTP/1.1")));
        }
    }

    #[tokio::test]
    async fn token_is_persisted_but_never_returned_in_status() {
        let dir = tempfile::tempdir().unwrap();
        let service = ClientUpdateService::new(dir.path().to_owned());
        service
            .configure(
                false,
                "https://vpn.example",
                None,
                Some("github_pat_TEST123"),
                None,
            )
            .await
            .unwrap();
        let restored = ClientUpdateService::new(dir.path().to_owned());
        assert_eq!(
            restored.record.lock().await.github_token,
            "github_pat_TEST123"
        );
        let status = serde_json::to_value(restored.status().await).unwrap();
        assert_eq!(status["github_token_set"], true);
        assert!(status.get("github_token").is_none());
        assert!(!status.to_string().contains("github_pat_TEST123"));
        restored
            .configure(true, "https://vpn.example", None, None, None)
            .await
            .unwrap();
        assert!(restored.status().await.github_token_set);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(dir.path().join("client-update-sync.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        restored
            .configure(true, "https://vpn.example", None, Some(""), None)
            .await
            .unwrap();
        assert!(
            !ClientUpdateService::new(dir.path().to_owned())
                .status()
                .await
                .github_token_set
        );
        assert!(normalize_token("secret\r\nAuthorization: injected").is_err());
    }

    #[tokio::test]
    async fn authenticated_api_client_does_not_follow_redirects() {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = [0; 4096];
            let n = socket.read(&mut bytes).await.unwrap();
            assert!(String::from_utf8_lossy(&bytes[..n]).contains("Bearer test_token"));
            socket.write_all(b"HTTP/1.1 302 Found\r\nLocation: https://example.invalid/leak\r\nContent-Length: 0\r\n\r\n").await.unwrap();
        });
        let client = github_client_for("", true).unwrap();
        let response = client
            .get(format!("http://{address}/api"))
            .bearer_auth("test_token")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::FOUND);
        server.await.unwrap();
        let download = github_client("")
            .unwrap()
            .get("https://github.com/test")
            .build()
            .unwrap();
        assert!(download
            .headers()
            .get(reqwest::header::AUTHORIZATION)
            .is_none());
    }

    #[test]
    fn minimum_version_uses_semantic_precedence() {
        for reported in [None, Some(""), Some("invalid")] {
            assert!(check_client_version("", reported).is_ok());
            assert!(check_client_version("0.1.32", reported).is_err());
        }
        for reported in ["0.1.31", "0.1.9", "0.1.32-beta.1"] {
            assert!(check_client_version("0.1.32", Some(reported)).is_err());
        }
        for reported in ["0.1.32", "v0.1.32", "0.1.32+build1", "0.1.100", "1.0.0"] {
            assert!(check_client_version("0.1.32", Some(reported)).is_ok());
        }
        assert!(check_client_version("corrupt", Some("99.0.0")).is_err());
    }

    #[tokio::test]
    async fn minimum_version_settings_survive_restart_and_can_be_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let service = ClientUpdateService::new(dir.path().to_owned());
        service
            .configure(false, "https://vpn.example", None, None, Some("v0.1.32"))
            .await
            .unwrap();
        let restored = ClientUpdateService::new(dir.path().to_owned());
        assert!(restored
            .enforce_client_version(Some("0.1.31"))
            .await
            .is_err());
        restored
            .configure(false, "https://vpn.example", None, None, None)
            .await
            .unwrap();
        assert_eq!(
            restored.status().await.record.minimum_client_version,
            "0.1.32"
        );
        assert!(restored
            .configure(false, "https://vpn.example", None, None, Some("oops"))
            .await
            .is_err());
        restored
            .configure(false, "https://vpn.example", None, None, Some(""))
            .await
            .unwrap();
        assert!(restored.enforce_client_version(None).await.is_ok());
    }

    fn fixture() -> (Manifest, Release, Vec<(String, Vec<u8>)>) {
        let mut manifest = Manifest {
            version: "0.1.21".into(),
            notes: "test".into(),
            pub_date: None,
            platforms: BTreeMap::new(),
            downloads: vec![],
        };
        let mut release = Release {
            tag_name: "v0.1.21".into(),
            draft: false,
            prerelease: false,
            assets: vec![],
        };
        let mut files = vec![];
        for (target, name, ext) in [
            ("windows-x86_64", "windows-amd64-setup", "exe"),
            ("darwin-x86_64", "macos-amd64", "app.tar.gz"),
            ("darwin-aarch64", "macos-arm64", "app.tar.gz"),
        ] {
            let name = format!("vpn-gui-{name}-v0.1.21.{ext}");
            let url = format!("https://github.com/{REPO}/releases/download/v0.1.21/{name}");
            let signature = STANDARD.encode([42; 100]);
            manifest.platforms.insert(
                target.into(),
                PlatformUpdate {
                    url,
                    signature: signature.clone(),
                },
            );
            for (filename, bytes) in [
                (name.clone(), b"package contents".to_vec()),
                (format!("{name}.sig"), signature.into_bytes()),
            ] {
                release.assets.push(Asset {
                    browser_download_url: format!(
                        "https://github.com/{REPO}/releases/download/v0.1.21/{filename}"
                    ),
                    size: bytes.len() as u64,
                    digest: Some(format!("sha256:{:x}", Sha256::digest(&bytes))),
                    name: filename.clone(),
                });
                files.push((filename, bytes));
            }
        }
        (manifest, release, files)
    }
    #[test]
    fn android_apk_is_optional_canonical_verified_and_aab_is_ignored() {
        let (_, mut release, _) = fixture();
        let original = package_assets(&release).unwrap().len();
        let apk = Asset {
            name: "vpn-android-universal-0.1.21.apk".into(),
            browser_download_url: format!("https://github.com/{REPO}/releases/download/v0.1.21/vpn-android-universal-0.1.21.apk"),
            size: 42,
            digest: Some(format!("sha256:{}", "ab".repeat(32))),
        };
        release.assets.push(apk.clone());
        let mut aab = apk.clone();
        aab.name = "vpn-android-universal-0.1.21.aab".into();
        release.assets.push(aab);
        assert_eq!(package_assets(&release).unwrap().len(), original + 1);
        release.assets.pop();
        release.assets.last_mut().unwrap().digest = None;
        assert!(package_assets(&release).is_err());
        *release.assets.last_mut().unwrap() = apk.clone();
        release.assets.last_mut().unwrap().size = 256 * 1024 * 1024 + 1;
        assert!(package_assets(&release).is_err());
        *release.assets.last_mut().unwrap() = apk.clone();
        release.assets.last_mut().unwrap().name = "vpn-android-universal-0.1.22.apk".into();
        assert!(package_assets(&release).is_err());
        *release.assets.last_mut().unwrap() = apk.clone();
        release.assets.push(apk);
        assert!(package_assets(&release).is_err());
    }
    #[test]
    fn rejects_incomplete_mismatched_or_foreign_release() {
        let (mut m, mut r, _) = fixture();
        assert!(validate(&m, &r).is_ok());
        m.platforms.get_mut("darwin-aarch64").unwrap().url = "http://127.0.0.1/private".into();
        assert!(validate(&m, &r).is_err());
        let (mut m, _, _) = fixture();
        m.platforms.remove("darwin-aarch64");
        assert!(validate(&m, &r).is_err());
        let (m, _, _) = fixture();
        r.tag_name = "v0.1.22".into();
        assert!(validate(&m, &r).is_err());
        assert!(version("0.1.22-beta.1").is_err());
        assert!(normalize_base("https://vpn.example:8443/path").is_err());
        assert_eq!(
            normalize_base("https://vpn.example:8443/").unwrap(),
            "https://vpn.example:8443"
        );
    }
    #[test]
    fn rejects_missing_digests_and_unsafe_paths() {
        let (_, mut r, _) = fixture();
        assert!(package_assets(&r).is_ok());
        r.assets[0].digest = None;
        assert!(package_assets(&r).is_err());
        assert!(local_path(
            Path::new("/tmp"),
            "https://vpn.example",
            "https://vpn.example/updates/releases/../../secrets",
            "secrets"
        )
        .is_none());
    }
    #[tokio::test]
    async fn publishes_local_urls_only_after_complete_verified_downloads() {
        let dir = tempfile::tempdir().unwrap();
        let service = ClientUpdateService::new(dir.path().to_owned());
        let (manifest, release, files) = fixture();
        let stage = dir.path().join("stage");
        tokio::fs::create_dir_all(&stage).await.unwrap();
        for (name, bytes) in &files {
            tokio::fs::write(stage.join(name), bytes).await.unwrap();
        }
        service
            .publish_downloaded(&stage, manifest.clone(), &release, "https://vpn.example")
            .await
            .unwrap();
        let published = service.current().await.unwrap().unwrap();
        let packages = package_assets(&release).unwrap();
        assert!(
            service
                .local_release_valid(&published, &manifest, &packages, "https://vpn.example")
                .await
        );
        for item in published.platforms.values() {
            assert!(item
                .url
                .starts_with("https://vpn.example/updates/releases/"));
        }
        use http_body_util::BodyExt;
        use tower::ServiceExt;
        let router = axum::Router::new().nest_service(
            "/updates",
            tower_http::services::ServeDir::new(dir.path().join("updates")),
        );
        let url = reqwest::Url::parse(&published.platforms["darwin-aarch64"].url).unwrap();
        let response = router
            .oneshot(
                axum::http::Request::builder()
                    .uri(url.path())
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        assert_eq!(
            &response.into_body().collect().await.unwrap().to_bytes()[..],
            b"package contents"
        );
        let previous = tokio::fs::read(service.manifest_path()).await.unwrap();
        tokio::fs::create_dir_all(&stage).await.unwrap();
        for (name, bytes) in &files {
            tokio::fs::write(stage.join(name), bytes).await.unwrap();
        }
        tokio::fs::write(stage.join(&files[0].0), b"tampered contents")
            .await
            .unwrap();
        assert!(service
            .publish_downloaded(&stage, manifest, &release, "https://vpn.example")
            .await
            .is_err());
        assert_eq!(
            previous,
            tokio::fs::read(service.manifest_path()).await.unwrap()
        );
        assert!(
            !service
                .local_release_valid(&published, &published, &packages, "https://other.example")
                .await
        );
    }
    #[tokio::test]
    async fn settings_survive_restart_and_concurrent_sync_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let service = Arc::new(ClientUpdateService::new(dir.path().to_owned()));
        service
            .configure(
                true,
                "https://vpn.example",
                Some("http://127.0.0.1:7897"),
                None,
                None,
            )
            .await
            .unwrap();
        let restored = ClientUpdateService::new(dir.path().to_owned());
        assert!(restored.status().await.record.auto_sync);
        assert_eq!(
            restored.status().await.record.public_base_url,
            "https://vpn.example"
        );
        assert_eq!(
            restored.status().await.record.proxy_url,
            "http://127.0.0.1:7897/"
        );
        restored
            .configure(true, "https://vpn.example", None, None, None)
            .await
            .unwrap();
        assert_eq!(
            restored.status().await.record.proxy_url,
            "http://127.0.0.1:7897/"
        );
        restored
            .configure(true, "https://vpn.example", Some(""), None, None)
            .await
            .unwrap();
        assert!(ClientUpdateService::new(dir.path().to_owned())
            .status()
            .await
            .record
            .proxy_url
            .is_empty());
        let _lock = service.gate.lock().await;
        assert!(service.queue().is_err());
        assert!(service
            .configure(false, "https://vpn.example", None, None, None)
            .await
            .is_err());
    }
    #[tokio::test]
    async fn downloads_over_http_and_rejects_corruption() {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = axum::Router::new().route(
            "/package",
            axum::routing::get(|| async { "real download bytes" }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let root = tempfile::tempdir().unwrap();
        let mut asset = Asset {
            name: "package".into(),
            browser_download_url: format!("http://{address}/package"),
            size: 19,
            digest: Some(format!(
                "sha256:{:x}",
                Sha256::digest(b"real download bytes")
            )),
        };
        let client = reqwest::Client::new();
        download_package(&client, &asset, &root.path().join("ok"))
            .await
            .unwrap();
        asset.digest = Some(format!("sha256:{}", "0".repeat(64)));
        assert!(
            download_package(&client, &asset, &root.path().join("bad-digest"))
                .await
                .is_err()
        );
        asset.size = 1;
        assert!(
            download_package(&client, &asset, &root.path().join("too-large"))
                .await
                .is_err()
        );
        asset.browser_download_url = format!("http://{address}/missing");
        assert!(
            download_package(&client, &asset, &root.path().join("missing"))
                .await
                .is_err()
        );
        server.abort();
    }
    #[tokio::test]
    #[ignore = "downloads real GitHub release assets; run manually with network access"]
    async fn live_github_mirror() {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let root = std::env::var("VPN_UPDATE_TEST_DIR").expect("isolated test directory required");
        let service = ClientUpdateService::new(PathBuf::from(root));
        service
            .configure(false, "http://127.0.0.1:18081", None, None, None)
            .await
            .unwrap();
        service.sync().await.unwrap();
        let status = service.status().await;
        assert!(status.record.last_error.is_none());
        assert_eq!(status.manifest.unwrap().platforms.len(), 3);
    }
}
