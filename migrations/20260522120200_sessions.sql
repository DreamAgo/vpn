-- Story 2.1: sessions 表
-- 存储 Refresh Token 的哈希，支持显式撤销

CREATE TABLE sessions (
    id TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NOT NULL,
    refresh_token_hash TEXT NOT NULL,
    ip_addr TEXT,
    user_agent TEXT,
    expires_at INTEGER NOT NULL,
    revoked_at INTEGER NULL,
    created_at INTEGER NOT NULL
);

CREATE UNIQUE INDEX idx_sessions_refresh_token_hash ON sessions(refresh_token_hash);
CREATE INDEX idx_sessions_user_id ON sessions(user_id);
CREATE INDEX idx_sessions_expires_at ON sessions(expires_at);
