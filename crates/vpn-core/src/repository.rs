//! Repository trait（具体 sqlx 实现由 vpn-server 提供）。
//!
//! 每个聚合根（user / peer / session / audit_log）一个 trait。
//! 后续 Story 按需填充每个 trait 的方法签名：
//! - Story 2.3：UserRepository、SessionRepository
//! - Story 4.5：PeerRepository
//! - Story 5.2：AuditLogRepository

// 占位：trait 方法签名将由后续 Story 添加，避免提前定义不准确的接口。
//
// 设计原则：
// - 所有方法 async（基于 sqlx）
// - 返回 Result<T, AppError>
// - 参数使用 newtype（如 UserId）而非裸 String
// - 不暴露 sqlx 特定类型（如 PgPool）到 trait 签名
