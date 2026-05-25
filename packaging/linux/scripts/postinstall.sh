#!/bin/sh
# nfpm postinstall：在 .deb / .rpm 安装与升级后运行。
# 作用：创建 vpn-server 系统用户、修正数据目录权限、reload + enable systemd 服务。
# 以 root 身份执行；保持 POSIX sh 兼容（无 bash 特性）。
set -e

SERVICE_USER="vpn-server"
SERVICE_GROUP="vpn-server"
DATA_DIR="/var/lib/vpn-server"
CONFIG_DIR="/etc/vpn-server"

# 1) 创建系统用户/组（幂等）。
if ! getent group "${SERVICE_GROUP}" >/dev/null 2>&1; then
    if command -v groupadd >/dev/null 2>&1; then
        groupadd --system "${SERVICE_GROUP}"
    elif command -v addgroup >/dev/null 2>&1; then
        addgroup --system "${SERVICE_GROUP}"
    fi
fi

if ! getent passwd "${SERVICE_USER}" >/dev/null 2>&1; then
    if command -v useradd >/dev/null 2>&1; then
        useradd --system --gid "${SERVICE_GROUP}" \
            --home-dir "${DATA_DIR}" --no-create-home \
            --shell /usr/sbin/nologin "${SERVICE_USER}"
    elif command -v adduser >/dev/null 2>&1; then
        adduser --system --ingroup "${SERVICE_GROUP}" \
            --home "${DATA_DIR}" --no-create-home \
            --shell /usr/sbin/nologin "${SERVICE_USER}"
    fi
fi

# 2) 数据目录权限（systemd StateDirectory 也会兜底，但首次安装先建好）。
mkdir -p "${DATA_DIR}"
chown -R "${SERVICE_USER}:${SERVICE_GROUP}" "${DATA_DIR}"
chmod 750 "${DATA_DIR}"

# 配置目录可读，env 文件含潜在敏感值则收紧权限。
if [ -f "${CONFIG_DIR}/vpn-server.env" ]; then
    chown root:"${SERVICE_GROUP}" "${CONFIG_DIR}/vpn-server.env"
    chmod 640 "${CONFIG_DIR}/vpn-server.env"
fi

# 3) systemd：reload 并 enable（不强制 start，交由管理员确认配置后再启动）。
if command -v systemctl >/dev/null 2>&1; then
    systemctl daemon-reload || true
    systemctl enable vpn-server.service || true
    echo "vpn-server 已安装并设置开机自启。"
    echo "请先编辑 /etc/vpn-server/vpn-server.env，然后执行： systemctl start vpn-server"
fi

exit 0
