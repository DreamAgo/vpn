# 部署指南

单机模式下，`vpn-server` 在同一进程运行 Web 后台与 WireGuard 数据平面。推荐 Docker 部署。

## 前置条件

- 一台有公网 IP 的 Linux 服务器（建议 1C1G 起步，50 节点规模 2C2G）。
- 一个解析到该服务器的域名（用于自动 HTTPS）。
- 开放端口：TCP 80/443（Web + ACME），UDP 47358（混淆数据平面）。内部 WireGuard 51820 不公开。
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
- UDP 47358 需在 compose 与宿主防火墙同时放行；启用方法与 PSK 生成见 [udp-obfuscation.md](udp-obfuscation.md)。

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

升级后服务端会把旧 `network_settings_v2`（或更早的 v1 MTU）迁移到 `network_settings_v3`，并仅在 v3 不存在时用数据面环境变量初始化基础 VPN、混淆、MTU 和 DNS。之后请在管理后台“网络设置”页面调整；非秘密环境变量不会覆盖已保存值。基础 VPN/混淆修改保存后，可在网络设置页面点击“重启并应用”（Linux/macOS 服务端支持）；在线连接会暂时中断，页面自动检查配置是否生效，LAN 路由和 DNS 转发规则热更新，MTU 与客户端 DNS 在下一次连接或重连时生效。混淆 PSK 仍只由 `VPN_OBFS_PSK` 提供。内置 DNS 的 UDP/TCP 53 只绑定 VPN 网关，不要添加 Docker 公网端口映射。

## 健康检查

- `GET /health` 返回 200 表示进程存活（compose 已配置 healthcheck）。

## 故障排查

- 看日志：`docker compose logs -f` 或 `journalctl -u vpn-server`。
- 客户端连不上：依次检查 UDP 47358 是否放行、“网络设置”中的混淆公网 Endpoint 是否为客户端可达地址、两端时钟偏差及节点在后台是否 `online`。v2 已初始化后修改 `VPN_OBFS_ENDPOINT` 不会覆盖后台保存值。
- 证书申请失败：确认域名解析正确且 80 端口可达。
- 真实隧道相关限制见 [REAL-HARDWARE-CHECKLIST.md](REAL-HARDWARE-CHECKLIST.md)。

## 分布式网关部署

控制面继续保存用户、设备、授权及 VPN IP，并运行内置网关 `local`。每个额外 Linux 网关有自己的客户端 WireGuard 密钥及公网地址，隧道流量通过专用 WireGuard 互联返回控制面，沿用原有访问控制和 DNS。以下以现有控制面加一台 `east` 网关为例。两台主机使用同一版本的服务端。

### 1. 配置私有互联

在控制面和 `east` 各创建一对专用于互联的 WireGuard 密钥。示例地址 `172.31.254.0/30` 须避开现有 VPN、站点和主机网段。每台额外网关在控制面占用独立的接口和地址段。控制面安装 `iproute2`、`wireguard-tools`、`nftables`，额外网关还需 `iputils-ping`。统一发行镜像包含这些工具。

控制面 `/etc/wireguard/wg-east.conf`：

```ini
[Interface]
Address = 172.31.254.1/30
ListenPort = 47950
PrivateKey = <控制面互联私钥>
Table = off

[Peer]
PublicKey = <east 互联公钥>
AllowedIPs = 0.0.0.0/0
```

`east` 主机 `/etc/wireguard/wg-control.conf`：

```ini
[Interface]
Address = 172.31.254.2/30
PrivateKey = <east 互联私钥>
Table = off

[Peer]
PublicKey = <控制面互联公钥>
Endpoint = <控制面互联地址>:47950
AllowedIPs = 0.0.0.0/0
PersistentKeepalive = 25
```

在两台主机启用 IPv4 转发，并使用宽松反向路径检查，以便流量通过专用路由表：

```sh
sudo sysctl -w net.ipv4.ip_forward=1 net.ipv4.conf.all.rp_filter=2
```

将上述值写入宿主机持久化 sysctl 配置。控制面启动 `wg-quick@wg-east`，额外网关启动 `wg-quick@wg-control`。防火墙允许这两台主机互联 UDP，在额外网关放行公网数据 UDP 及探测 UDP。额外网关使用路由规则优先级 10200 和 10201、路由表 47359 和 47360；控制面使用路由协议编号 186。在启用前确保这些值可用。

### 2. 登记额外网关

为 `east` 另建客户端 WireGuard 密钥对。下文的客户端密钥与上一节的互联密钥分别使用。为每个额外网关生成不同的管理令牌，例如在权限受限的目录执行 `umask 077; openssl rand -hex 32 > token`。在控制面用权限为 0600 的 JSON 文件保存网关目录：

```json
[
  {
    "id": "east",
    "name": "东区网关",
    "token": "<east 的 64 位小写十六进制令牌>",
    "server_public_key": "<east 客户端 WireGuard 公钥>",
    "server_endpoint": "east.vpn.example.com:51820",
    "probe_endpoint": "east.vpn.example.com:47359",
    "transit_interface": "wg-east",
    "transit_local_ip": "172.31.254.1",
    "transit_peer_ip": "172.31.254.2"
  }
]
```

在控制面的现有服务环境中追加：

```dotenv
VPN_BIND_ADDR=127.0.0.1:8080
VPN_WG_BACKEND=kernel
VPN_CLUSTER_GATEWAYS_FILE=/etc/yilian/cluster-gateways.json
VPN_CLUSTER_PROBE_BIND_ADDR=0.0.0.0:47359
VPN_CLUSTER_PROBE_ENDPOINT=vpn.example.com:47359
```

控制面使用已有 VPN 网段，继续开放原数据 UDP，再开放探测 UDP 47359。集群控制面的专用 WireGuard 互联接口须和服务端进程位于同一网络命名空间。集群模式要求控制面的 HTTP 监听在回环地址，通过 HTTPS 反向代理提供后台和客户端 API。启用内置网关的 UDP 混淆时，还须保留用于校验混淆配置的 `VPN_HTTPS=true` 和 `VPN_DOMAIN`。

只在独立的私有 HTTPS 管理入口转发网关同步路径 `/api/v1/cluster/agents/*/sync`。HTTPS 服务器证书按主机名校验，每台网关使用自己的令牌。需要双向 TLS 时，在私有反向代理上校验客户端证书。VPN 互联负责数据面和 ICMP 就绪探测。

使用 [控制面 Compose 示例](../docker/docker-compose.cluster-controller.yml) 时，将上述 JSON 保存为同级目录的 `controller-secrets/gateways.json`，将原有服务环境和新增变量写入 `controller.env`。确认生效的 WireGuard 后端为 `kernel`。若在现有实例上启用集群，挂载现有数据卷以保留账户、配置和密钥。

控制面也可使用 `docker/docker-compose.cluster-controller.yml`。把网关目录保存为同级的 `controller-secrets/gateways.json`，在 `controller.env` 中沿用当前服务的网络和登录配置，设置 `VPN_CLUSTER_PROBE_BIND_ADDR`、`VPN_CLUSTER_PROBE_ENDPOINT` 和 `VPN_WG_BACKEND=kernel`。服务绑定地址须能被本机 HTTPS 代理访问。迁移现有实例时，把 compose 数据卷指向已有数据卷，保留数据库、密钥和已保存的网络设置，并确认后台实际生效的 WireGuard 后端为 kernel。

### 3. 启动额外网关

在 `east` 的部署目录放置 `docker/docker-compose.gateway.yml`，并在同级目录创建 `gateway-secrets/token` 和 `gateway-secrets/client-wg.key`，所有者设为 root，文件权限设为 0600。创建同级的 `gateway.env`：

```dotenv
VPN_GATEWAY_ID=east
VPN_GATEWAY_CONTROL_URL=https://vpn-control.example.internal
VPN_GATEWAY_TRANSIT_INTERFACE=wg-control
VPN_GATEWAY_TRANSIT_HEALTH_ADDR=172.31.254.1
VPN_GATEWAY_PROBE_BIND_ADDR=0.0.0.0:47359
VPN_SUBNET=10.8.0.0/24
VPN_WG_INTERFACE=yilian-wg
VPN_WG_BACKEND=kernel
VPN_LISTEN_PORT=51820
```

`VPN_SUBNET` 须与控制面实际生效的 VPN 子网一致。私有 CA 及双向 TLS 分别可通过 `VPN_GATEWAY_CA_FILE` 和 `VPN_GATEWAY_TLS_IDENTITY_FILE` 指向挂载目录中的 PEM 文件。客户端证书文件同时包含客户端证书链与私钥。部署时在 `VPN_IMAGE` 填入含此功能的统一发行镜像，然后运行：

```sh
docker compose -f docker-compose.gateway.yml up -d
docker compose -f docker-compose.gateway.yml logs -f
```

使用二进制部署时，在服务环境中加入 `VPN_GATEWAY_MODE=agent`，设置同样的变量，并将 `VPN_GATEWAY_TOKEN_FILE` 和 `VPN_GATEWAY_WG_KEY_FILE` 改为宿主机路径。一个网络命名空间运行一个额外网关。宿主机可通过 `wg show wg-control` 检查互联，控制面界面显示网关是否已应用快照并就绪。

启用 UDP 混淆时，在额外网关环境追加 `VPN_OBFS_ENABLED=true`、`VPN_OBFS_ENDPOINT=east.vpn.example.com:47358` 和 `VPN_GATEWAY_OBFS_PSK_FILE=/run/vpn-gateway-secrets/obfs.psk`。在对应的控制面网关条目加入 `transport` 对象，字段为 `protocol: "obfs-v1"`、`mode: "low-overhead-v1"`、`endpoint: "east.vpn.example.com:47358"`、`psk` 和 `path_mtu: 1500`。端点、PSK、模式和路径 MTU 须与额外网关一致。混淆时放行公网 UDP 47358 和探测 UDP 47359，原生 WireGuard 端口只供本机代理使用。

### 4. 授权与切换验证

1. 在后台保留内置网关为默认。用两台同账号设备连接，确认初始均使用默认网关。
2. 只给其中一台设备授予 `east` 权限。在后台将它的目标网关改为 `east`，等待客户端心跳和 WireGuard 握手。检查实际网关变为 `east`、VPN IP 不变，并检查已有权限下的 DNS、对端及站点路由。
3. 停止额外网关或阻断它的互联流量，确认客户端在授权范围内恢复。撤销额外网关权限，确认客户端切回默认网关，旧网关的转发租约失效。
4. 恢复自动选路后，分别验证额外网关延迟更低及公网探测可达但数据 UDP 不通的情况，检查客户端以 WireGuard 握手结果确定最终连接。

同一个站点前缀只能分配给一台设备。启用集群前先清理完全重复的站点路由。重叠但长度不同的网段仍按最长前缀匹配。飞书审批到期沿用原有网络策略，收回审批授予的业务网段访问。控制面失联时，现有网关只能在短租约内继续转发。全局默认网关变更后，旧客户端需重新连接以获取新默认网关。

退役网关前，若该网关为默认，先在管理后台改为另一台就绪的默认网关。停止退役网关的进程，并在控制面关闭其专用 WireGuard 互联接口，再从网关目录移除该条目并重启。恢复单机部署时，先停止全部额外网关并拆除控制面的专用互联接口，再取消 `VPN_CLUSTER_GATEWAYS_FILE` 并重启。服务端会清理集群转发规则和租约，沿用原数据库中的设备和 VPN IP。
