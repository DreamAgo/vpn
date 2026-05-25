# 部署指南

`vpn-server` 是单进程服务，内嵌 Web 后台与 WireGuard 数据平面。推荐 Docker 部署。

## 前置条件

- 一台有公网 IP 的 Linux 服务器（建议 1C1G 起步，50 节点规模 2C2G）。
- 一个解析到该服务器的域名（用于自动 HTTPS）。
- 开放端口：TCP 80/443（Web + ACME），UDP 51820（WireGuard）。
- 内核支持 WireGuard 数据面所需的 TUN 与 `CAP_NET_ADMIN` 权限。

## 方式一：Docker（推荐）

```bash
git clone https://github.com/OWNER/vpn.git
cd vpn

# 配置环境变量
cp docker/.env.example .env
$EDITOR .env        # 至少设置 VPN_HTTPS=true 与 VPN_DOMAIN

docker compose -f docker/docker-compose.yml up -d
docker compose -f docker/docker-compose.yml logs -f
```

容器要点：

- 以非 root 运行，但需 `CAP_NET_ADMIN`（compose 已声明）创建/配置网络接口。
- 数据卷挂载 `VPN_DATA_DIR` 与数据库目录，确保重启/升级后密钥与数据不丢。
- UDP 51820 需在 compose 与宿主防火墙同时放行。

首次访问 `https://<VPN_DOMAIN>`，按**首次配置向导**创建首位管理员账号。

## 方式二：二进制 / 系统服务

适用于不便用 Docker 的环境。安装包脚手架见 [`packaging/`](../packaging)。

```bash
# 构建
cargo build --release --workspace      # 产物 target/release/vpn-server
cd frontend && npm ci && npm run build  # 前端已由 rust-embed 内嵌进二进制

# 运行（示例）
sudo VPN_HTTPS=true VPN_DOMAIN=vpn.example.com \
     DATABASE_URL=sqlite:///var/lib/vpn-server/vpn.db?mode=rwc \
     VPN_DATA_DIR=/var/lib/vpn-server \
     ./target/release/vpn-server
```

生产建议注册为 systemd **system** 服务（含 `AmbientCapabilities=CAP_NET_ADMIN`），模板见 `packaging/linux/systemd/vpn-server.service`。

## HTTPS / 证书

- `VPN_HTTPS=true` 时使用 `rustls-acme` 自动向 Let's Encrypt 申请并续期证书，缓存于 `VPN_DATA_DIR`。
- 需保证 80 端口可达（HTTP-01 校验）。
- 开发环境可设 `VPN_HTTPS=false` 走明文 8080。

## 升级

1. 拉取新镜像 / 替换二进制。
2. 重启服务——启动时自动执行数据库 migration、加载已有 WireGuard 服务端密钥、从 `peers` 表恢复节点。
3. 数据卷保持不变即可平滑升级。

## 健康检查

- `GET /health` 返回 200 表示进程存活（compose 已配置 healthcheck）。

## 故障排查

- 看日志：`docker compose logs -f` 或 `journalctl -u vpn-server`。
- 客户端连不上：依次检查 UDP 51820 是否放行、`VPN_ENDPOINT` 是否为客户端可达的公网地址、节点在后台是否 `online`。
- 证书申请失败：确认域名解析正确且 80 端口可达。
- 真实隧道相关限制见 [REAL-HARDWARE-CHECKLIST.md](REAL-HARDWARE-CHECKLIST.md)。
