-- Story 4.1 / 4.3: system_config 通用 KV 表
-- 用于持久化服务端级别的配置/状态（首个用途：服务端 WireGuard 密钥对）。
-- key 主键，value 文本，updated_at 毫秒时间戳。

CREATE TABLE system_config (
    key TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);
