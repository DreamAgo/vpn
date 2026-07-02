-- 节点健康监控：
-- 1) peers 增加健康指标列（在线起点 / 心跳 RTT / 丢包率 / 客户端版本）；
-- 2) peer_events 变更记录表（OS / IP / Endpoint / 设备名 / 版本变化）。

-- 本次转为在线的起始时刻（unix ms）；离线/强制下线时清空。用于计算“在线时长”。
ALTER TABLE peers ADD COLUMN online_since INTEGER NULL;
-- 客户端最近一次心跳测得的往返延迟（毫秒，客户端上报）。
ALTER TABLE peers ADD COLUMN rtt_ms INTEGER NULL;
-- 客户端最近 N 次心跳的失败率（百分比 0-100，客户端上报）。
ALTER TABLE peers ADD COLUMN loss_pct REAL NULL;
-- 客户端版本（注册时上报，如 "0.1.0"）。
ALTER TABLE peers ADD COLUMN client_version TEXT NULL;

-- 节点属性变更记录。无外键：peer/用户被物理删除后仍保留历史（视图用 LEFT JOIN 解析名称）。
CREATE TABLE peer_events (
    id TEXT PRIMARY KEY NOT NULL,
    peer_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    -- 变更字段：'os_info' | 'endpoint' | 'vpn_ip' | 'device_name' | 'client_version'
    field TEXT NOT NULL,
    old_value TEXT NULL,
    new_value TEXT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX idx_peer_events_peer ON peer_events (peer_id, created_at DESC);
CREATE INDEX idx_peer_events_created ON peer_events (created_at DESC);
