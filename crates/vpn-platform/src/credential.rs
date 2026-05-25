//! Story 4.9: 跨平台凭证存储抽象。
//!
//! 设计：
//! - 主路径 [`KeyringCredentialStore`]：基于跨三平台的 [`keyring`] crate
//!   （macOS Keychain / Linux libsecret / Windows Credential Manager）。
//! - 降级 / 可测路径 [`FileCredentialStore`]：当系统凭证服务不可用（例如 Linux
//!   headless 无 libsecret）时使用，凭证落盘前用 XSalsa20Poly1305
//!   （NaCl `crypto_secretbox`）加密，密钥派生自用户主目录路径
//!   `sha256(home_path)` 的前 32 字节。
//! - **可测纯逻辑**：[`FileCredentialStore`] 的 save→load→delete round-trip、
//!   密文不含明文、KDF 确定性，均在本机完整单测。
//! - **需真机验证**：[`KeyringCredentialStore`] 真正读写系统钥匙串需要桌面
//!   会话 / 解锁的钥匙串，单测以 `#[ignore]` 标注。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use xsalsa20poly1305::aead::{Aead, KeyInit};
use xsalsa20poly1305::{Nonce, XSalsa20Poly1305};

use crate::error::{PlatformError, Result};

/// 默认 keyring service 名（条目命名空间）。
pub const DEFAULT_SERVICE: &str = "vpn-cli";

/// 跨平台凭证存储 trait。
pub trait CredentialStore: Send + Sync {
    /// 保存（或覆盖）一条 `key -> value` 凭证。
    fn save(&self, key: &str, value: &str) -> Result<()>;

    /// 读取凭证；不存在返回 `Ok(None)`。
    fn load(&self, key: &str) -> Result<Option<String>>;

    /// 删除凭证；不存在视为成功（幂等）。
    fn delete(&self, key: &str) -> Result<()>;
}

// ===========================================================================
// 主路径：keyring crate（跨三平台系统钥匙串）
// ===========================================================================

/// 基于系统钥匙串的凭证存储（macOS Keychain / Linux libsecret / Windows CredMgr）。
pub struct KeyringCredentialStore {
    service: String,
}

impl KeyringCredentialStore {
    /// 使用默认 service 名 [`DEFAULT_SERVICE`]。
    pub fn new() -> Self {
        Self {
            service: DEFAULT_SERVICE.to_string(),
        }
    }

    /// 使用自定义 service 名（条目命名空间）。
    pub fn with_service(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }

    fn entry(&self, key: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(&self.service, key)
            .map_err(|e| PlatformError::Credential(format!("创建 keyring 条目失败: {e}")))
    }
}

impl Default for KeyringCredentialStore {
    fn default() -> Self {
        Self::new()
    }
}

impl CredentialStore for KeyringCredentialStore {
    fn save(&self, key: &str, value: &str) -> Result<()> {
        self.entry(key)?
            .set_password(value)
            .map_err(|e| PlatformError::Credential(format!("写入钥匙串失败: {e}")))
    }

    fn load(&self, key: &str) -> Result<Option<String>> {
        match self.entry(key)?.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(PlatformError::Credential(format!("读取钥匙串失败: {e}"))),
        }
    }

    fn delete(&self, key: &str) -> Result<()> {
        match self.entry(key)?.delete_credential() {
            Ok(()) => Ok(()),
            Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(PlatformError::Credential(format!(
                "删除钥匙串条目失败: {e}"
            ))),
        }
    }
}

// ===========================================================================
// 降级 / 可测路径：加密文件
// ===========================================================================

/// 加密文件凭证存储。
///
/// 文件格式（JSON 之外刻意自描述、零依赖解析）：每行一条
/// `key\tbase64(nonce(24B) || ciphertext)`；value 在加密后才落盘。
pub struct FileCredentialStore {
    path: PathBuf,
    cipher: XSalsa20Poly1305,
}

impl FileCredentialStore {
    /// 在默认路径 `~/.config/vpn-cli/creds.enc` 上创建文件存储。
    ///
    /// 加密密钥派生自用户主目录路径。
    pub fn new() -> Result<Self> {
        let home = dirs::home_dir().ok_or_else(|| {
            PlatformError::Credential("无法定位用户主目录，文件降级存储不可用".to_string())
        })?;
        let path = Self::default_path()?;
        Ok(Self::with_path_and_home(path, &home))
    }

    /// 推导默认凭证文件路径 `~/.config/vpn-cli/creds.enc`。
    pub fn default_path() -> Result<PathBuf> {
        let base = dirs::config_dir()
            .ok_or_else(|| PlatformError::Credential("无法定位配置目录".to_string()))?;
        Ok(base.join("vpn-cli").join("creds.enc"))
    }

    /// 显式指定文件路径与「主目录」（密钥派生输入）。便于测试隔离。
    pub fn with_path_and_home(path: impl Into<PathBuf>, home: &Path) -> Self {
        let key = derive_key_from_home(home);
        let cipher = XSalsa20Poly1305::new((&key).into());
        Self {
            path: path.into(),
            cipher,
        }
    }

    /// 当前后端使用的文件路径。
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn read_all(&self) -> Result<HashMap<String, String>> {
        let mut map = HashMap::new();
        let content = match std::fs::read_to_string(&self.path) {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(map),
            Err(e) => return Err(PlatformError::Io(e)),
        };
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let (key, enc) = line.split_once('\t').ok_or_else(|| {
                PlatformError::Crypto("凭证文件格式损坏（缺少分隔符）".to_string())
            })?;
            let plain = self.decrypt(enc)?;
            map.insert(key.to_string(), plain);
        }
        Ok(map)
    }

    fn write_all(&self, map: &HashMap<String, String>) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = String::new();
        for (key, value) in map {
            if key.contains('\t') || key.contains('\n') {
                return Err(PlatformError::InvalidArgument(
                    "凭证 key 不能包含制表符或换行".to_string(),
                ));
            }
            let enc = self.encrypt(value)?;
            out.push_str(key);
            out.push('\t');
            out.push_str(&enc);
            out.push('\n');
        }
        std::fs::write(&self.path, out)?;
        // 尽力收紧权限（仅 unix 生效）。
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    fn encrypt(&self, plaintext: &str) -> Result<String> {
        use base64::Engine;
        let nonce_bytes = random_nonce();
        let nonce = Nonce::from(nonce_bytes);
        let ct = self
            .cipher
            .encrypt(&nonce, plaintext.as_bytes())
            .map_err(|_| PlatformError::Crypto("加密失败".to_string()))?;
        let mut blob = Vec::with_capacity(nonce_bytes.len() + ct.len());
        blob.extend_from_slice(&nonce_bytes);
        blob.extend_from_slice(&ct);
        Ok(base64::engine::general_purpose::STANDARD.encode(blob))
    }

    fn decrypt(&self, b64: &str) -> Result<String> {
        use base64::Engine;
        let blob = base64::engine::general_purpose::STANDARD
            .decode(b64.trim())
            .map_err(|e| PlatformError::Crypto(format!("base64 解码失败: {e}")))?;
        if blob.len() < 24 {
            return Err(PlatformError::Crypto("密文过短".to_string()));
        }
        let (nonce_bytes, ct) = blob.split_at(24);
        let nonce_arr: [u8; 24] = nonce_bytes
            .try_into()
            .map_err(|_| PlatformError::Crypto("nonce 长度异常".to_string()))?;
        let nonce = Nonce::from(nonce_arr);
        let plain = self
            .cipher
            .decrypt(&nonce, ct)
            .map_err(|_| PlatformError::Crypto("解密失败（密钥不匹配或数据损坏）".to_string()))?;
        String::from_utf8(plain).map_err(|e| PlatformError::Crypto(format!("明文非 UTF-8: {e}")))
    }
}

impl CredentialStore for FileCredentialStore {
    fn save(&self, key: &str, value: &str) -> Result<()> {
        let mut map = self.read_all()?;
        map.insert(key.to_string(), value.to_string());
        self.write_all(&map)
    }

    fn load(&self, key: &str) -> Result<Option<String>> {
        let map = self.read_all()?;
        Ok(map.get(key).cloned())
    }

    fn delete(&self, key: &str) -> Result<()> {
        let mut map = self.read_all()?;
        map.remove(key);
        self.write_all(&map)
    }
}

/// 从主目录路径派生 32 字节对称密钥：`sha256(home_path_bytes)`。
fn derive_key_from_home(home: &Path) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(home.to_string_lossy().as_bytes());
    let digest = hasher.finalize();
    let mut key = [0u8; 32];
    key.copy_from_slice(&digest[..32]);
    key
}

/// 生成 24 字节随机 nonce（XSalsa20Poly1305 使用 24B nonce）。
fn random_nonce() -> [u8; 24] {
    use xsalsa20poly1305::aead::rand_core::RngCore;
    let mut nonce = [0u8; 24];
    xsalsa20poly1305::aead::OsRng.fill_bytes(&mut nonce);
    nonce
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn make_store(dir: &std::path::Path) -> FileCredentialStore {
        let path = dir.join("creds.enc");
        FileCredentialStore::with_path_and_home(path, dir)
    }

    #[test]
    fn round_trip_save_load_delete() {
        let dir = tempdir().unwrap();
        let store = make_store(dir.path());

        assert_eq!(store.load("token").unwrap(), None);

        store.save("token", "secret-value-123").unwrap();
        assert_eq!(
            store.load("token").unwrap(),
            Some("secret-value-123".to_string())
        );

        // 覆盖。
        store.save("token", "new-value").unwrap();
        assert_eq!(store.load("token").unwrap(), Some("new-value".to_string()));

        store.delete("token").unwrap();
        assert_eq!(store.load("token").unwrap(), None);

        // 幂等删除。
        store.delete("token").unwrap();
    }

    #[test]
    fn multiple_keys_independent() {
        let dir = tempdir().unwrap();
        let store = make_store(dir.path());
        store.save("a", "alpha").unwrap();
        store.save("b", "beta").unwrap();
        assert_eq!(store.load("a").unwrap(), Some("alpha".to_string()));
        assert_eq!(store.load("b").unwrap(), Some("beta".to_string()));
        store.delete("a").unwrap();
        assert_eq!(store.load("a").unwrap(), None);
        assert_eq!(store.load("b").unwrap(), Some("beta".to_string()));
    }

    #[test]
    fn ciphertext_does_not_contain_plaintext() {
        let dir = tempdir().unwrap();
        let store = make_store(dir.path());
        let secret = "super-sensitive-PLAINTEXT-marker";
        store.save("k", secret).unwrap();

        let raw = std::fs::read(store.path()).unwrap();
        // 明文 marker 不得出现在落盘字节中。
        assert!(
            !raw.windows(secret.len()).any(|w| w == secret.as_bytes()),
            "明文泄漏到密文文件"
        );
    }

    #[test]
    fn nonce_randomized_per_write() {
        let dir = tempdir().unwrap();
        let store = make_store(dir.path());
        store.save("k", "same").unwrap();
        let first = std::fs::read_to_string(store.path()).unwrap();
        store.save("k", "same").unwrap();
        let second = std::fs::read_to_string(store.path()).unwrap();
        // 相同明文两次写入应因随机 nonce 而产生不同密文。
        assert_ne!(first, second);
    }

    #[test]
    fn wrong_key_cannot_decrypt() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("creds.enc");
        let store_a =
            FileCredentialStore::with_path_and_home(&path, std::path::Path::new("/home/userA"));
        store_a.save("k", "value").unwrap();

        // 不同 home → 不同派生密钥 → 解密应失败。
        let store_b =
            FileCredentialStore::with_path_and_home(&path, std::path::Path::new("/home/userB"));
        assert!(store_b.load("k").is_err());
    }

    #[test]
    fn kdf_is_deterministic() {
        let k1 = derive_key_from_home(std::path::Path::new("/home/alice"));
        let k2 = derive_key_from_home(std::path::Path::new("/home/alice"));
        let k3 = derive_key_from_home(std::path::Path::new("/home/bob"));
        assert_eq!(k1, k2);
        assert_ne!(k1, k3);
    }

    #[test]
    fn default_path_ends_with_expected() {
        let p = FileCredentialStore::default_path().unwrap();
        assert!(p.ends_with("vpn-cli/creds.enc"));
    }

    #[test]
    #[ignore = "真机验证：需要桌面会话 / 已解锁的系统钥匙串"]
    fn keyring_round_trip() {
        let store = KeyringCredentialStore::with_service("vpn-cli-test");
        store.save("token", "v").unwrap();
        assert_eq!(store.load("token").unwrap(), Some("v".to_string()));
        store.delete("token").unwrap();
        assert_eq!(store.load("token").unwrap(), None);
    }
}
