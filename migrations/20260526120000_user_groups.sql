-- 用户组 + 组级"可路由网段"(访问控制)
--
-- 一个用户组持有一组可路由 CIDR(逗号分隔)。成员注册时,服务端据此计算其
-- WireGuard allowed_routes:VPN 子网 + 本组网段(+站点网关 LAN)。未分组用户回退
-- 到全局 server_routes(system_config 的 server_routes)。

CREATE TABLE user_groups (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    routes TEXT NOT NULL DEFAULT '', -- 逗号分隔的 CIDR 列表,如 "10.0.0.0/8,172.31.100.0/24"
    created_at INTEGER NOT NULL,     -- unix ms
    updated_at INTEGER NOT NULL
);

CREATE UNIQUE INDEX idx_user_groups_name ON user_groups(name);

-- users 增加所属组(可空)。删组时由应用层把成员该列清空(SET NULL)。
ALTER TABLE users ADD COLUMN group_id TEXT NULL;
CREATE INDEX idx_users_group_id ON users(group_id);
