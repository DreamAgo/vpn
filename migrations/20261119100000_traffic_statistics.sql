-- Traffic measurements are operational history. Keep them when a peer is purged.
CREATE TABLE traffic_sources (
    id TEXT PRIMARY KEY,
    generation TEXT NOT NULL,
    sequence INTEGER NOT NULL,
    sampled_at INTEGER NOT NULL,
    first_sample_at INTEGER NOT NULL
);
CREATE TABLE traffic_counters (
    source_id TEXT NOT NULL,
    public_key TEXT NOT NULL,
    peer_id TEXT NOT NULL,
    generation TEXT NOT NULL,
    rx INTEGER NOT NULL,
    tx INTEGER NOT NULL,
    active INTEGER NOT NULL DEFAULT 1,
    upload_bps REAL,
    download_bps REAL,
    sampled_at INTEGER NOT NULL,
    PRIMARY KEY(source_id, public_key)
);
CREATE TABLE traffic_totals (
    peer_id TEXT PRIMARY KEY,
    device_name TEXT NOT NULL,
    username TEXT NOT NULL,
    vpn_ip TEXT NOT NULL,
    upload_bytes INTEGER NOT NULL DEFAULT 0,
    download_bytes INTEGER NOT NULL DEFAULT 0,
    sampled_at INTEGER NOT NULL
);
CREATE TABLE traffic_hourly (
    hour INTEGER NOT NULL,
    peer_id TEXT NOT NULL,
    upload_bytes INTEGER NOT NULL DEFAULT 0,
    download_bytes INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY(hour, peer_id)
);
