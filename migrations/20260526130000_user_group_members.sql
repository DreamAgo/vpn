-- 用户↔组 多对多关联(一个用户可属多个组)。
--
-- 取代早先 users.group_id 的单组关系:成员的 allowed_routes 取其所有组可路由网段的并集
-- (访问控制;无任何组则回退全局 server_routes)。users.group_id 列保留但不再使用。

CREATE TABLE user_group_members (
    user_id  TEXT NOT NULL,
    group_id TEXT NOT NULL,
    PRIMARY KEY (user_id, group_id)
);

CREATE INDEX idx_ugm_group ON user_group_members(group_id);
CREATE INDEX idx_ugm_user ON user_group_members(user_id);

-- 迁移既有单组关系到关联表。
INSERT INTO user_group_members (user_id, group_id)
    SELECT id, group_id FROM users WHERE group_id IS NOT NULL;
