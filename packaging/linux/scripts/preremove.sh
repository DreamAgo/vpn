#!/bin/sh
# nfpm preremove：在 .deb / .rpm 卸载前运行（升级时也可能触发，需判断参数）。
# 作用：停止并 disable systemd 服务。
# 保持 POSIX sh 兼容。
set -e

# 卸载时 deb 传 "remove"，rpm 传 "0"。升级（upgrade）时不应彻底清理，这里仅停服务。
if command -v systemctl >/dev/null 2>&1; then
    systemctl stop vpn-server.service || true
    systemctl disable vpn-server.service || true
fi

# 注意：不删除 /var/lib/vpn-server（数据）与 /etc/vpn-server（配置），
# 以免升级或重装丢失数据。彻底清理由管理员手动或 purge 流程处理。

exit 0
