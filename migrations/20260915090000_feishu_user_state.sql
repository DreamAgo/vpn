-- 飞书状态与本地管理员禁用分别保存，恢复通讯录状态不会覆盖人工管理。
CREATE TABLE feishu_user_states (
    subject TEXT PRIMARY KEY NOT NULL,
    app_id TEXT NOT NULL,
    open_id TEXT,
    directory_user_id TEXT,
    name TEXT NOT NULL DEFAULT '',
    email TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT 'unknown',
    blocked INTEGER NOT NULL DEFAULT 0 CHECK(blocked IN (0,1)),
    synced_at INTEGER,
    attempted_at INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    last_event_at INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX idx_feishu_state_open_id ON feishu_user_states(open_id);
CREATE TABLE feishu_contact_inbox (
    event_id TEXT PRIMARY KEY NOT NULL,
    app_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    id_type TEXT NOT NULL,
    event_type TEXT NOT NULL,
    event_at INTEGER NOT NULL,
    payload_hash TEXT NOT NULL,
    done INTEGER NOT NULL DEFAULT 0,
    attempted_at INTEGER NOT NULL DEFAULT 0,
    last_error TEXT
);
