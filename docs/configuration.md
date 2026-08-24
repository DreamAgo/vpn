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
| `VPN_OBFS_ENABLED` | `false` | 启用并强制使用 `obfs-v1` UDP 混淆；要求 HTTPS。 |
| `VPN_OBFS_PSK` | 无 | 32 字节标准 Base64 PSK；启用混淆时必填。 |
| `VPN_OBFS_MODE` | `low-overhead-v1` | 混淆模式，也可设 `paranoid-v1`。 |
| `VPN_OBFS_BIND_ADDR` | `0.0.0.0:47358` | 混淆公网监听地址。 |
| `VPN_OBFS_ENDPOINT` | `<VPN_DOMAIN>:47358` | 下发给新客户端的混淆 endpoint。 |
| `VPN_OBFS_PATH_MTU` | `1500` | 外层路径 MTU（576–9000）。 |
| `VPN_WG_BACKEND` | `noop` | WireGuard 数据平面后端：`kernel` / `userspace` / `auto` / `noop`。**生产必须显式设置**（默认 `noop` 不建真实隧道）。详见下节。 |
| `VPN_WG_INTERFACE` | `wg0` | WireGuard 接口名（`kernel`/`userspace` 后端创建的接口）。 |
| `VPN_AUDIT_RETENTION_DAYS` | `180` | 审计日志保留天数，超期由后台任务自动清理。 |
| `VPN_FEISHU_APPROVAL_OPTIONS_TOKEN` | （无） | 飞书审批“关联外部选项”请求校验 token。至少 32 个字符；应使用独立高熵随机值。 |
| `VPN_FEISHU_APPROVAL_CODE` | （无） | 只接受该审批定义 Code 的网络授权审批。与下列五项全部配置时启用审批 webhook。 |
| `VPN_FEISHU_APPROVAL_GROUP_CONTROL_ID` | （无） | “网络组”控件的稳定 ID；值必须是 `user_groups` 外部选项返回的用户组 ID。 |
| `VPN_FEISHU_APPROVAL_EXPIRY_CONTROL_ID` | （无） | “授权到期日期”控件的稳定 ID。到期日按上海时区次日 00:00 保存为独占 `expires_at`。 |
| `VPN_FEISHU_APPROVAL_REASON_CONTROL_ID` | （无） | “申请事由”控件的稳定 ID。 |
| `VPN_FEISHU_APPROVAL_VERIFICATION_TOKEN` | （无） | 飞书事件订阅 Verification Token（敏感值）。 |
| `VPN_FEISHU_APPROVAL_ENCRYPT_KEY` | （无） | 飞书事件订阅 Encrypt Key（敏感值）；服务端只接受验签成功的加密事件。 |
| `RUST_LOG` | `info` | 日志级别（tracing EnvFilter 语法），如 `vpn_server=debug,info`。 |

## 启动校验

- `VPN_HTTPS=true` 但缺少 `VPN_DOMAIN` → 启动失败并报错。
- 数据目录与数据库父目录会在启动时自动创建。
- 未配置 `VPN_FEISHU_APPROVAL_OPTIONS_TOKEN` 时，飞书审批外部选项接口返回 HTTP 503。
- `VPN_FEISHU_APPROVAL_OPTIONS_TOKEN` 少于 32 个字符时启动失败。
- 飞书审批配置只设置一部分时启动失败；启用审批时还必须完整配置飞书 App ID、App Secret 和 HTTPS Redirect URI，确保审批创建的账号可以通过飞书登录；Verification Token 少于 16 字符或 Encrypt Key 少于 16 字符时启动失败。
- 启用飞书审批网络授权时会强制探测 `nft` 并在恢复 kernel peer 前安装 ACL；缺少 `nftables`、权限不足或规则失败时拒绝启动，不会退化为仅下发客户端路由。未启用审批的既有 kernel 部署保持原行为。

## WireGuard 数据平面后端（`VPN_WG_BACKEND`）

服务端真实隧道由该后端决定。**默认 `noop` 不建隧道**，生产务必显式设置。

| 取值 | 行为 | 依赖 | 适用 |
|---|---|---|---|
| `kernel` | 用内核 WireGuard；配置审批网络授权后启用强制 ACL | **内核 WG 模块** + `CAP_NET_ADMIN` + `wireguard-tools`；审批另需 `nftables` | 当前审批授权支持的生产模式 |
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
- 审批授权首期只支持显式 `kernel`。`userspace`/`auto` 不启用审批 ACL；不要在这些后端配置飞书审批事件环境变量。
- ACL 使用独立 `inet yilian_vpn_acl` table，只处理 `iifname=wg0` 的 FORWARD 流量，不修改 INPUT、OUTPUT 或 NAT。授权以短租约刷新；服务异常后租约自动到期并停止业务访问。

## 端口

| 端口 | 协议 | 用途 |
|---|---|---|
| `VPN_BIND_ADDR` 端口（默认 8080） | TCP | HTTP API + Web 后台 |
| 80 / 443 | TCP | 启用 HTTPS 时：80 用于 ACME HTTP-01 + 跳转，443 用于 Web/API |
| `VPN_OBFS_BIND_ADDR`（默认 47358） | UDP | 唯一公网混淆数据平面；51820 仅容器内部使用 |

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
