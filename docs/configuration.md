# 配置参考

`vpn-server` 全部配置通过**环境变量**注入，无需配置文件。下表与 `crates/vpn-server/src/config.rs` 一致。

## 服务端环境变量

| 变量 | 默认值 | 说明 |
|---|---|---|
| `VPN_BIND_ADDR` | `0.0.0.0:8080` | HTTP/HTTPS 监听地址。启用 HTTPS 时通常配合 80/443。 |
| `DATABASE_URL` | `sqlite://./dev.db?mode=rwc` | SQLite 数据库 URL。生产建议指向数据卷，如 `sqlite:///var/lib/vpn-server/vpn.db?mode=rwc`。 |
| `VPN_HTTPS` | `false` | `true`/`1` 启用自动 HTTPS（ACME）。启用时**必须**设置 `VPN_DOMAIN`。 |
| `VPN_DOMAIN` | （无） | 公网域名，用于 ACME 证书申请。`VPN_HTTPS=true` 时必填。 |
| `VPN_DATA_DIR` | `./data` | 数据目录：JWT 私钥、ACME 证书缓存等。生产建议持久化卷。 |
| `VPN_SUBNET` | `10.8.0.0/24` | VPN 虚拟子网（CIDR）。`.1` 预留给服务端，`.2` 起分配给节点。 |
| `VPN_LISTEN_PORT` | `51820` | WireGuard UDP 监听端口。 |
| `VPN_ENDPOINT` | `<VPN_DOMAIN 或 127.0.0.1>:<VPN_LISTEN_PORT>` | 客户端连接服务端用的 `host:port`。多数情况留空由域名推导即可。 |
| `VPN_WG_BACKEND` | `noop` | WireGuard 数据平面后端：`kernel` / `userspace` / `auto` / `noop`。**生产必须显式设置**（默认 `noop` 不建真实隧道）。详见下节。 |
| `VPN_WG_INTERFACE` | `wg0` | WireGuard 接口名（`kernel`/`userspace` 后端创建的接口）。 |
| `VPN_AUDIT_RETENTION_DAYS` | `180` | 审计日志保留天数，超期由后台任务自动清理。 |
| `RUST_LOG` | `info` | 日志级别（tracing EnvFilter 语法），如 `vpn_server=debug,info`。 |

## 启动校验

- `VPN_HTTPS=true` 但缺少 `VPN_DOMAIN` → 启动失败并报错。
- 数据目录与数据库父目录会在启动时自动创建。

## WireGuard 数据平面后端（`VPN_WG_BACKEND`）

服务端真实隧道由该后端决定。**默认 `noop` 不建隧道**，生产务必显式设置。

| 取值 | 行为 | 依赖 | 适用 |
|---|---|---|---|
| `kernel` | 用内核 WireGuard（`ip link add type wireguard`） | **内核 WG 模块** + `CAP_NET_ADMIN` + `wireguard-tools` | 现代 Linux，性能最佳 |
| `userspace` | 用用户态 `wireguard-go` | 仅 `/dev/net/tun` + `wireguard-go`（镜像已内置） | 无内核 WG 的老内核（如 CentOS 7） |
| `auto` | 先试 `kernel`，失败回退 `userspace` | 同上两者取其一 | 一套配置通吃新旧机器 |
| `noop` | 仅记账、不建隧道 | 无 | 开发 / 无特权环境 / 演示 |

`userspace` 性能低于 `kernel`（用户态加解密 + 包拷贝，CPU 占用高数倍），小规格 VPS 高负载下更明显；能上内核就优先 `kernel`。`auto` 在现代机器仍走内核态，只有内核不支持时才降级，日志会写明**实际采用**了哪个（`mode=Kernel`/`Userspace`）。

### 内核是否自带 WireGuard

WireGuard 自 **Linux 5.6（2020-03）** 并入主线，之后的内核默认带该模块。

| 发行版 | 内核 WG |
|---|---|
| CentOS 7（3.10） | ❌ 无（须 `userspace` 或升级系统） |
| CentOS/RHEL 8 | ⚠️ 8.4/8.5+ 回填 |
| Rocky/Alma/RHEL 9 | ✅ |
| Debian 11+ / Ubuntu 20.04+ | ✅ |
| 任意内核 ≥ 5.6 | ✅ |

> RHEL 系是回填，版本号低不代表没有；以实测为准：
> ```bash
> modprobe wireguard && echo "有内核 WG" || echo "无 → 用 userspace"
> ```

### 运行要点

- `kernel`/`userspace` 都需容器 `--cap-add NET_ADMIN`；`userspace` 额外需 `--device /dev/net/tun`。
- 接口名由 `VPN_WG_INTERFACE`（默认 `wg0`）决定。

## 端口

| 端口 | 协议 | 用途 |
|---|---|---|
| `VPN_BIND_ADDR` 端口（默认 8080） | TCP | HTTP API + Web 后台 |
| 80 / 443 | TCP | 启用 HTTPS 时：80 用于 ACME HTTP-01 + 跳转，443 用于 Web/API |
| `VPN_LISTEN_PORT`（默认 51820） | UDP | WireGuard 数据平面 |

## 最小生产配置示例

```bash
VPN_HTTPS=true
VPN_DOMAIN=vpn.example.com
DATABASE_URL=sqlite:///var/lib/vpn-server/vpn.db?mode=rwc
VPN_DATA_DIR=/var/lib/vpn-server
VPN_SUBNET=10.8.0.0/24
VPN_LISTEN_PORT=51820
VPN_WG_BACKEND=auto        # 现代机器走内核态，老内核(如 CentOS 7)自动回退用户态
RUST_LOG=info
```
