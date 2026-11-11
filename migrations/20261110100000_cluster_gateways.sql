CREATE TABLE cluster_devices (
    peer_id TEXT PRIMARY KEY REFERENCES peers(id) ON DELETE CASCADE,
    public_key TEXT NOT NULL UNIQUE,
    credential_hash BLOB NOT NULL,
    target_gateway_id TEXT,
    revision INTEGER NOT NULL DEFAULT 1,
    confirmed_gateway_id TEXT,
    confirmed_revision INTEGER
);

CREATE TABLE cluster_device_grants (
    peer_id TEXT NOT NULL REFERENCES cluster_devices(peer_id) ON DELETE CASCADE,
    gateway_id TEXT NOT NULL,
    PRIMARY KEY (peer_id, gateway_id)
);

-- Keep the fencing deadline even if an administrator deletes the peer.
CREATE TABLE cluster_remote_leases (
    vpn_ip TEXT PRIMARY KEY,
    peer_id TEXT NOT NULL UNIQUE,
    public_key TEXT NOT NULL,
    gateway_id TEXT NOT NULL,
    client_until INTEGER NOT NULL,
    issued_until INTEGER NOT NULL DEFAULT 0,
    draining INTEGER NOT NULL DEFAULT 0 CHECK (draining IN (0, 1))
);

CREATE TABLE cluster_local_leases (
    vpn_ip TEXT PRIMARY KEY,
    peer_id TEXT NOT NULL UNIQUE REFERENCES peers(id) ON DELETE CASCADE,
    public_key TEXT NOT NULL,
    client_until INTEGER NOT NULL
);
