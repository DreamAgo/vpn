#!/bin/sh
# wg-forward-setup.sh —— 由 vpn-server.service 的 ExecStartPost 调用。
# 作用：为异地组网（站点 LAN 互通 / 全隧道出公网）打开内核转发并放行防火墙规则。
#   1) sysctl net.ipv4.ip_forward=1（IPv4 转发）。
#   2) iptables FORWARD 放行同一 wg 接口上 peer 之间 / 站点之间互转发
#      （装了 Docker 的主机 FORWARD 链默认 DROP，不放行则站点间流量被丢）。
#   3) 可选：VPN_NAT=1 时，对 VPN_SUBNET 出口网卡做 MASQUERADE（全隧道出公网）。
#
# 仅在 kernel 后端下需要（VPN_WG_BACKEND=kernel）；noop 后端直接跳过。
# 设计为幂等（先 -C 检测再 -I）且 best-effort（缺工具不致命，避免拖垮服务启动）。
# 保持 POSIX sh 兼容（无 bash 特性）。

set -u

# --- 读取环境（systemd 已注入 EnvironmentFile，这里再给出脚手架默认值）---
WG_BACKEND="${VPN_WG_BACKEND:-noop}"
WG_IFACE="${VPN_WG_INTERFACE:-wg0}"
VPN_SUBNET="${VPN_SUBNET:-10.8.0.0/24}"
VPN_NAT="${VPN_NAT:-0}"
VPN_NAT_EGRESS="${VPN_NAT_EGRESS:-}"

log() { echo "[wg-forward-setup] $*"; }

# 非 kernel 后端无需操作内核数据平面。
if [ "${WG_BACKEND}" != "kernel" ]; then
    log "VPN_WG_BACKEND=${WG_BACKEND}（非 kernel），跳过转发/防火墙配置。"
    exit 0
fi

# --- 1) 开启 IPv4 转发（幂等）---
if command -v sysctl >/dev/null 2>&1; then
    sysctl -w net.ipv4.ip_forward=1 >/dev/null 2>&1 \
        || log "警告：sysctl net.ipv4.ip_forward=1 失败（可能缺权限）。"
else
    # 退化路径：直接写 procfs。
    echo 1 > /proc/sys/net/ipv4/ip_forward 2>/dev/null \
        || log "警告：无法写入 /proc/sys/net/ipv4/ip_forward。"
fi

# --- 2) 放行 wg 接口上的互转发（幂等：先 -C 检测，缺规则再 -I 插入）---
if command -v iptables >/dev/null 2>&1; then
    if ! iptables -C FORWARD -i "${WG_IFACE}" -o "${WG_IFACE}" -j ACCEPT 2>/dev/null; then
        iptables -I FORWARD -i "${WG_IFACE}" -o "${WG_IFACE}" -j ACCEPT 2>/dev/null \
            && log "已放行 FORWARD ${WG_IFACE} <-> ${WG_IFACE}。" \
            || log "警告：插入 FORWARD ${WG_IFACE} 互转发规则失败。"
    else
        log "FORWARD ${WG_IFACE} 互转发规则已存在，跳过。"
    fi

    # --- 3) 可选 MASQUERADE（全隧道出公网）---
    if [ "${VPN_NAT}" = "1" ]; then
        egress="${VPN_NAT_EGRESS}"
        if [ -z "${egress}" ] && command -v ip >/dev/null 2>&1; then
            # 自动探测默认路由出口网卡。
            egress=$(ip route show default 2>/dev/null | awk '/default/ {for (i=1;i<=NF;i++) if ($i=="dev") {print $(i+1); exit}}')
        fi
        if [ -n "${egress}" ]; then
            if ! iptables -t nat -C POSTROUTING -s "${VPN_SUBNET}" -o "${egress}" -j MASQUERADE 2>/dev/null; then
                iptables -t nat -A POSTROUTING -s "${VPN_SUBNET}" -o "${egress}" -j MASQUERADE 2>/dev/null \
                    && log "已对 ${VPN_SUBNET} 出 ${egress} 启用 MASQUERADE。" \
                    || log "警告：插入 MASQUERADE 规则失败。"
            else
                log "MASQUERADE ${VPN_SUBNET} -> ${egress} 已存在，跳过。"
            fi
        else
            log "警告：VPN_NAT=1 但无法确定出口网卡（设置 VPN_NAT_EGRESS），跳过 MASQUERADE。"
        fi
    fi
else
    log "警告：未找到 iptables，跳过 FORWARD/NAT 放行（站点间转发可能被丢弃）。"
fi

# best-effort：永远以成功退出，避免拖垮 vpn-server 启动。
exit 0
