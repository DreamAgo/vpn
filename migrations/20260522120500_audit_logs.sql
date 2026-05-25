-- Story 5.1: audit_logs 表
-- 记录所有写操作（POST/PATCH/DELETE）与关键安全事件（登录失败等）。

CREATE TABLE audit_logs (
    id TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NULL,
    username TEXT NULL,
    action TEXT NOT NULL,
    resource TEXT NOT NULL,
    ip_addr TEXT NULL,
    user_agent TEXT NULL,
    -- 结构化补充信息（JSON 文本）
    metadata TEXT NULL,
    -- HTTP 状态码（中间件写入；显式事件可为 NULL）
    status_code INTEGER NULL,
    created_at INTEGER NOT NULL
);

CREATE INDEX idx_audit_logs_created_at ON audit_logs(created_at);
CREATE INDEX idx_audit_logs_user_id ON audit_logs(user_id);
CREATE INDEX idx_audit_logs_action ON audit_logs(action);
