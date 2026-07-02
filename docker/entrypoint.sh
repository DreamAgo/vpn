#!/bin/sh
# 容器内：开启转发 + 把 VPN 客户端流量 MASQUERADE 进 docker 网络（作为站点网关）
sysctl -w net.ipv4.ip_forward=1 2>/dev/null || true
SUB="${VPN_SUBNET:-10.8.0.0/24}"
iptables -t nat -C POSTROUTING -s "$SUB" ! -d "$SUB" -j MASQUERADE 2>/dev/null \
  || iptables -t nat -A POSTROUTING -s "$SUB" ! -d "$SUB" -j MASQUERADE
echo "[entrypoint] ip_forward=$(cat /proc/sys/net/ipv4/ip_forward), MASQUERADE for $SUB ready"
exec /usr/local/bin/vpn-server
