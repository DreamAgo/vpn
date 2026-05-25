-- Story 4.x（异地组网）：peers 增加 routed_subnets
-- 该 peer 背后路由的 LAN 网段（站点内网），逗号分隔的 CIDR 列表，如 "192.168.10.0/24,192.168.11.0/24"。
-- 服务端据此把这些网段加入该 peer 的 WireGuard allowed-ips（cryptokey routing）。

ALTER TABLE peers ADD COLUMN routed_subnets TEXT NOT NULL DEFAULT '';
