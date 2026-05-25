-- Story 5.5: peers.status 新增 'force_removed' 状态
-- SQLite 不支持直接修改 CHECK 约束，采用「建新表 → 复制数据 → drop 旧表 → rename」方式重建。
-- 保留所有列 / 索引 / 外键。

CREATE TABLE peers_new (
    id TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NOT NULL,
    device_name TEXT NOT NULL,
    wg_public_key TEXT NOT NULL,
    vpn_ip TEXT NOT NULL,
    endpoint TEXT NULL,
    os_info TEXT NULL,
    last_seen_at INTEGER NULL,
    status TEXT NOT NULL CHECK (status IN ('online', 'offline', 'deleted', 'force_removed')) DEFAULT 'offline',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    FOREIGN KEY (user_id) REFERENCES users(id)
);

INSERT INTO peers_new (id, user_id, device_name, wg_public_key, vpn_ip, endpoint,
                       os_info, last_seen_at, status, created_at, updated_at)
SELECT id, user_id, device_name, wg_public_key, vpn_ip, endpoint,
       os_info, last_seen_at, status, created_at, updated_at
FROM peers;

DROP TABLE peers;

ALTER TABLE peers_new RENAME TO peers;

CREATE UNIQUE INDEX idx_peers_wg_public_key ON peers(wg_public_key);
CREATE UNIQUE INDEX idx_peers_vpn_ip ON peers(vpn_ip);
CREATE INDEX idx_peers_user_id ON peers(user_id);
CREATE INDEX idx_peers_status ON peers(status);
CREATE INDEX idx_peers_last_seen_at ON peers(last_seen_at);
