-- 飞书审批网络授权：历史账号默认 legacy，新建飞书账号显式 approval_required。
ALTER TABLE users ADD COLUMN access_mode TEXT NOT NULL DEFAULT 'legacy'
    CHECK (access_mode IN ('legacy', 'approval_required'));

-- Webhook 先持久化再 ACK。event_id 同时兼容 v2 event_id 与旧事件 uuid。
CREATE TABLE feishu_approval_inbox (
    event_id TEXT PRIMARY KEY NOT NULL,
    instance_code TEXT NOT NULL,
    payload_hash TEXT NOT NULL,
    payload TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'processing', 'done', 'retry', 'rejected')),
    attempts INTEGER NOT NULL DEFAULT 0,
    next_attempt_at INTEGER NOT NULL,
    last_error TEXT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX idx_feishu_inbox_work ON feishu_approval_inbox(status, next_attempt_at);
CREATE UNIQUE INDEX idx_feishu_inbox_instance_payload
    ON feishu_approval_inbox(instance_code, payload_hash);
CREATE UNIQUE INDEX idx_feishu_inbox_instance ON feishu_approval_inbox(instance_code);

-- 每个审批实例独立保存到期日；人工 user_group_members 不受影响。
CREATE TABLE access_grants (
    id TEXT PRIMARY KEY NOT NULL,
    approval_instance_code TEXT NOT NULL UNIQUE,
    user_id TEXT NOT NULL,
    group_id TEXT NOT NULL,
    expires_at INTEGER NOT NULL,
    reason TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE,
    FOREIGN KEY(group_id) REFERENCES user_groups(id) ON DELETE CASCADE
);
CREATE INDEX idx_access_grants_user_expiry ON access_grants(user_id, expires_at);
CREATE INDEX idx_access_grants_group_expiry ON access_grants(group_id, expires_at);
