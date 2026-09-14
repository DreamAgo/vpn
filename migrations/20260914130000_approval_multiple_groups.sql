-- 一个审批实例可以授予多个用户组，保留旧授权及逐组幂等约束。
CREATE TABLE access_grants_multi (
    id TEXT PRIMARY KEY NOT NULL,
    approval_instance_code TEXT NOT NULL,
    user_id TEXT NOT NULL,
    group_id TEXT NOT NULL,
    expires_at INTEGER NOT NULL,
    reason TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (approval_instance_code, group_id),
    FOREIGN KEY(user_id) REFERENCES users(id) ON DELETE CASCADE,
    FOREIGN KEY(group_id) REFERENCES user_groups(id) ON DELETE CASCADE
);
INSERT INTO access_grants_multi
    SELECT id, approval_instance_code, user_id, group_id, expires_at, reason, created_at, updated_at
    FROM access_grants;
DROP TABLE access_grants;
ALTER TABLE access_grants_multi RENAME TO access_grants;
CREATE INDEX idx_access_grants_user_expiry ON access_grants(user_id, expires_at);
CREATE INDEX idx_access_grants_group_expiry ON access_grants(group_id, expires_at);
