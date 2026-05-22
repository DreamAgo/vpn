//! 业务服务 trait（具体实现由 vpn-server 提供）。

use crate::Result;
use async_trait::async_trait;
use std::fmt::Debug;

/// 密码哈希服务（argon2id 实现见 vpn-server）。
pub trait PasswordHasher: Send + Sync + Debug {
    /// 把明文密码哈希为安全的 PHC 字符串。
    fn hash(&self, plaintext: &str) -> Result<String>;

    /// 校验明文密码与哈希是否匹配（常量时间）。
    fn verify(&self, plaintext: &str, hash: &str) -> Result<bool>;
}

/// JWT 签发与验证服务。
#[async_trait]
pub trait TokenIssuer: Send + Sync + Debug {
    /// 为 user_id + role 签发 Access Token（短期）。
    async fn issue_access(&self, user_id: &str, role: &str) -> Result<String>;

    /// 签发 Refresh Token（长期）。
    async fn issue_refresh(&self, user_id: &str) -> Result<String>;

    /// 解析并验证 Access Token，返回 (user_id, role)。
    async fn verify_access(&self, token: &str) -> Result<(String, String)>;

    /// 解析并验证 Refresh Token，返回 user_id。
    async fn verify_refresh(&self, token: &str) -> Result<String>;
}

/// ID 生成服务（UUID v7 实现见 vpn-server）。
pub trait IdGenerator: Send + Sync + Debug {
    /// 生成一个新 ID。
    fn new_id(&self) -> String;
}

#[cfg(test)]
mod tests {
    /// Trait 仅定义接口，单元测试在实现 crate（vpn-server）中编写。
    /// 此处仅做编译检查。
    use super::*;

    #[derive(Debug, Default)]
    struct DummyHasher;

    impl PasswordHasher for DummyHasher {
        fn hash(&self, _: &str) -> Result<String> {
            Ok("dummy".to_string())
        }
        fn verify(&self, _: &str, _: &str) -> Result<bool> {
            Ok(true)
        }
    }

    #[test]
    fn password_hasher_trait_compiles() {
        let h: Box<dyn PasswordHasher> = Box::new(DummyHasher);
        let hash = h.hash("x").unwrap();
        assert_eq!(hash, "dummy");
    }
}
