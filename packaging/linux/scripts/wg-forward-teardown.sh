#!/bin/sh
# wg-forward-teardown.sh —— 由 vpn-server.service 的 ExecStopPost 调用。
# 作用：清理 wg-forward-setup.sh 插入的 iptables 规则（best-effort）。
#   - 删除 FORWARD wg <-> wg 互转发 ACCEPT 规则。
#   - 若启用过 NAT，删除 VPN_SUBNET 的 MASQUERADE 规则。
# 不还原 net.ipv4.ip_forward（系统级开关，可能被其他服务依赖，不擅自关闭）。
#
# 全程 best-effort：规则不存在 / 缺工具均不报错，永远成功退出。
# 保持 POSIX sh 兼容。

set -u

WG_BACKEND="${VPN_WG_BACKEND:-noop}"
WG_IFACE="${VPN_WG_INTERFACE:-wg0}"
VPN_SUBNET="${VPN_SUBNET:-10.8.0.0/24}"
VPN_NAT="${VPN_NAT:-0}"
VPN_NAT_EGRESS="${VPN_NAT_EGRESS:-}"

log() { echo "[wg-forward-teardown] $*"; }

if [ "${WG_BACKEND}" != "kernel" ]; then
    exit 0
fi

if command -v iptables >/dev/null 2>&1; then
    # 删除互转发规则（重复 -D 直到不存在，清掉可能的多份）。
    while iptables -C FORWARD -i "${WG_IFACE}" -o "${WG_IFACE}" -j ACCEPT 2>/dev/null; do
        iptables -D FORWARD -i "${WG_IFACE}" -o "${WG_IFACE}" -j ACCEPT 2>/dev/null || break
    done

    if [ "${VPN_NAT}" = "1" ]; then
        egress="${VPN_NAT_EGRESS}"
        if [ -z "${egress}" ] && command -v ip >/dev/null 2>&1; then
            egress=$(ip route show default 2>/dev/null | awk '/default/ {for (i=1;i<=NF;i++) if ($i=="dev") {print $(i+1); exit}}')
        fi
        if [ -n "${egress}" ]; then
            while iptables -t nat -C POSTROUTING -s "${VPN_SUBNET}" -o "${egress}" -j MASQUERADE 2>/dev/null; do
                iptables -t nat -D POSTROUTING -s "${VPN_SUBNET}" -o "${egress}" -j MASQUERADE 2>/dev/null || break
            done
        fi
    fi
    log "iptables 规则清理完成（best-effort）。"
fi

exit 0
