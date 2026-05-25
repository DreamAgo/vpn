-- Story 4.3: peers 表
-- 存储每个用户注册的 WireGuard 节点（设备）。
-- 一个 user 可有多个 peer，但 vpn_ip 静态绑定、wg_public_key 全局唯一。

CREATE TABLE peers (
    id TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NOT NULL,
    device_name TEXT NOT NULL,
    wg_public_key TEXT NOT NULL,
    vpn_ip TEXT NOT NULL,
    endpoint TEXT NULL,
    os_info TEXT NULL,
    last_seen_at INTEGER NULL,
    status TEXT NOT NULL CHECK (status IN ('online', 'offline', 'deleted')) DEFAULT 'offline',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    FOREIGN KEY (user_id) REFERENCES users(id)
);

CREATE UNIQUE INDEX idx_peers_wg_public_key ON peers(wg_public_key);
CREATE UNIQUE INDEX idx_peers_vpn_ip ON peers(vpn_ip);
CREATE INDEX idx_peers_user_id ON peers(user_id);
CREATE INDEX idx_peers_status ON peers(status);
CREATE INDEX idx_peers_last_seen_at ON peers(last_seen_at);
