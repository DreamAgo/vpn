# 配置参考

`vpn-server` 的启动基础设施通过环境变量注入；数据面和飞书集成参数只在对应聚合配置尚不存在时读取环境变量，随后存入数据库并通过管理后台修改。下表共 44 项：`ServerConfig` 读取的 43 项，以及 tracing 直接读取的 `RUST_LOG`。

## 服务端环境变量

| 变量 | 默认值 | 说明 |
|---|---|---|
| `VPN_BIND_ADDR` | `0.0.0.0:8080` | HTTP/HTTPS 监听地址。启用 HTTPS 时通常配合 80/443。 |
| `DATABASE_URL` | `sqlite://./dev.db?mode=rwc` | SQLite 数据库 URL。生产建议指向数据卷，如 `sqlite:///var/lib/vpn-server/vpn.db?mode=rwc`。 |
| `VPN_HTTPS` | `false` | `true`/`1` 表示部署入口使用 HTTPS，并启用相关策略校验；当前进程监听仍为 HTTP，须由反向代理终止 TLS。启用时**必须**设置 `VPN_DOMAIN`。 |
| `VPN_DOMAIN` | （无） | HTTPS 公网域名及部分 endpoint 的默认主机。`VPN_HTTPS=true` 时必填。 |
| `VPN_DATA_DIR` | `./data` | 数据目录：JWT 私钥等。生产建议持久化卷。 |
| `VPN_SUBNET` | `10.8.0.0/24` | VPN 虚拟子网（CIDR）。仅在数据库无 `network_settings_v3` 时作为一次性种子。`.1` 预留给服务端（含内置 DNS），`.2` 起分配给节点。 |
| `VPN_LISTEN_PORT` | `51820` | WireGuard UDP 监听端口。 |
| `VPN_ENDPOINT` | `<VPN_DOMAIN 或 127.0.0.1>:<VPN_LISTEN_PORT>` | 客户端连接服务端用的 `host:port`。多数情况留空由域名推导即可。 |
| `VPN_OBFS_ENABLED` | `false` | 启用并强制使用 `obfs-v1` UDP 混淆；要求 HTTPS。 |
| `VPN_OBFS_PSK` | 无 | 32 字节标准 Base64 PSK；启用混淆时必填。 |
| `VPN_OBFS_MODE` | `low-overhead-v1` | 混淆模式，也可设 `paranoid-v1`。 |
| `VPN_OBFS_BIND_ADDR` | `0.0.0.0:47358` | 混淆公网监听地址。 |
| `VPN_OBFS_ENDPOINT` | `<VPN_DOMAIN>:47358` | 首次初始化时下发给客户端的混淆 endpoint；之后在“网络设置”修改。 |
| `VPN_OBFS_PATH_MTU` | `1500` | 外层路径 MTU（576–9000）。 |
| `VPN_TUN_MTU_MODE` | `fixed` | 首次初始化隧道 MTU 模式：`fixed` / `auto`。数据库已有 `network_settings_v3` 后忽略。 |
| `VPN_TUN_MTU_DEFAULT` | `1360` | 首次初始化默认 MTU。 |
| `VPN_TUN_MTU_MIN` | `1280` | 首次初始化自动模式下限。 |
| `VPN_TUN_MTU_MAX` | `1420` | 首次初始化自动模式上限。 |
| `VPN_WG_BACKEND` | `noop` | WireGuard 数据平面后端：`kernel` / `userspace` / `auto` / `noop`。**生产必须显式设置**（默认 `noop` 不建真实隧道）。详见下节。 |
| `VPN_WG_INTERFACE` | `wg0` | 首次初始化 WireGuard 接口名；之后在“网络设置”修改并重启生效。 |
| `VPN_SERVER_ROUTES` | （空） | 首次初始化服务端 LAN 路由，逗号分隔 CIDR；之后在“网络设置”维护。 |
| `VPN_DNS_MODE` | `disabled` | 首次初始化客户端 DNS 策略：`disabled` / `global`。旧值 `split` 兼容解释为 `global`。 |
| `VPN_DNS_DEFAULT_UPSTREAMS` | （空） | 首次初始化默认上游，逗号分隔 IPv4:53，如 `223.5.5.5:53,1.1.1.1:53`。 |
| `VPN_DNS_FORWARD_RULES` | `[]` | 首次初始化后缀转发规则 JSON 数组，如 `[{"domain":"corp.example.com","upstreams":["10.0.0.53:53"]}]`。 |
| `VPN_DNS_STATIC_RECORDS` | `[]` | 首次初始化静态 A 记录 JSON 数组，如 `[{"name":"api.corp.example.com","address":"10.0.0.10","ttl":300}]`。 |
| `VPN_AUDIT_RETENTION_DAYS` | `180` | 审计日志保留天数，超期由后台任务自动清理。 |
| `VPN_NOTIFY_EMAIL_ENABLED` | `false` | 邮件通知的一次性初始开关；之后在“通知设置”维护。 |
| `VPN_SMTP_HOST` | （无） | 邮件通知的一次性 SMTP 主机。 |
| `VPN_SMTP_PORT` | `587` | 邮件通知的一次性 SMTP 端口。 |
| `VPN_SMTP_USERNAME` | （无） | 邮件通知的一次性 SMTP 用户名。 |
| `VPN_SMTP_PASSWORD` | （无） | 邮件通知的一次性 SMTP 密码。 |
| `VPN_NOTIFY_EMAIL_FROM` | （无） | 邮件通知的一次性发件人。 |
| `VPN_NOTIFY_EMAIL_TO` | （空） | 邮件通知的一次性收件人列表，逗号分隔。 |
| `VPN_FEISHU_APP_ID` | （无） | 飞书登录 App ID；仅在 DB 无集成聚合配置时初始化。 |
| `VPN_FEISHU_APP_SECRET` | （无） | 飞书登录 App Secret（敏感）；仅首次初始化，API 永不回显。 |
| `VPN_FEISHU_REDIRECT_URI` | （无） | 飞书 OAuth HTTPS 回调；固定路径 `/api/v1/auth/feishu/callback`。 |
| `VPN_FEISHU_APPROVAL_OPTIONS_TOKEN` | （无） | 飞书审批“关联外部选项”请求校验 token。至少 32 个字符；应使用独立高熵随机值。 |
| `VPN_FEISHU_APPROVAL_CODE` | （无） | 只接受该审批定义 Code 的网络授权审批。与下列五项全部配置时启用审批 webhook。 |
| `VPN_FEISHU_APPROVAL_GROUP_CONTROL_ID` | （无） | “网络组”控件的稳定 ID；支持单选或多选；值必须是 `user_groups` 外部选项返回的用户组 ID，不能使用组名称。 |
| `VPN_FEISHU_APPROVAL_EXPIRY_CONTROL_ID` | （无） | “授权到期日期”控件的稳定 ID。到期日按上海时区次日 00:00 保存为独占 `expires_at`。 |
| `VPN_FEISHU_APPROVAL_REASON_CONTROL_ID` | （无） | “申请事由”控件的稳定 ID。 |
| `VPN_FEISHU_APPROVAL_VERIFICATION_TOKEN` | （无） | 飞书事件订阅 Verification Token（敏感值）。 |
| `VPN_FEISHU_APPROVAL_ENCRYPT_KEY` | （无） | 飞书事件订阅 Encrypt Key（敏感值）；服务端只接受验签成功的加密事件。 |
| `RUST_LOG` | `info` | 日志级别（tracing EnvFilter 语法），如 `vpn_server=debug,info`。 |

> `RUST_LOG` 由 tracing 运行库读取，不计入上述 `ServerConfig` 的 43 项；它和监听、数据库、数据目录、审计保留期及系统密钥继续属于部署层配置。

## 启动校验

- `VPN_HTTPS=true` 但缺少 `VPN_DOMAIN` → 启动失败并报错。
- 数据目录会在启动时自动创建；SQLite 数据库父目录目前必须由部署提前创建。
- 未配置 `VPN_FEISHU_APPROVAL_OPTIONS_TOKEN` 时，飞书审批外部选项接口返回 HTTP 503。
- `VPN_FEISHU_APPROVAL_OPTIONS_TOKEN` 少于 32 个字符时启动失败。
- 飞书审批配置只设置一部分时启动失败；启用审批时还必须完整配置飞书 App ID、App Secret 和 HTTPS Redirect URI，确保审批创建的账号可以通过飞书登录；Verification Token 少于 16 字符或 Encrypt Key 少于 16 字符时启动失败。
- 启用飞书审批网络授权时会强制探测 `nft` 并在恢复 kernel peer 前安装 ACL；缺少 `nftables`、权限不足或规则失败时拒绝启动，不会退化为仅下发客户端路由。未启用审批的既有 kernel 部署保持原行为。
- 数据库尚无 v3 网络参数时，服务会迁移已有 `network_settings_v2`（再向前兼容 v1 MTU），并将 `VPN_SUBNET`、`VPN_LISTEN_PORT`、`VPN_ENDPOINT`、`VPN_WG_BACKEND`、`VPN_WG_INTERFACE`、`VPN_OBFS_*`、`VPN_DNS_*` 与 MTU 环境变量作为一次性种子。整组 JSON 落库后，非秘密环境变量变化不会覆盖后台配置；存量 JSON 损坏时拒绝启动，不会静默回退。`VPN_OBFS_PSK` 始终只从秘密环境变量读取，API 不返回其内容。
- 启用混淆传输时还会按 `VPN_OBFS_MODE` 与 `VPN_OBFS_PATH_MTU` 计算安全内层上限：`fixed` 的默认值、`auto` 的最小值不得超过该上限。路径连 1280 都无法承载时拒绝启动；后台保存同样返回明确校验错误，避免客户端重连后才失败。

## 集成设置

管理员可在“集成设置”页面维护飞书登录、飞书审批和审批外部选项 Token。环境变量只在数据库没有 `integration_settings_v1` 时原子初始化一次；此后修改环境变量不会覆盖页面保存的 desired 配置。所有修改均在服务端重启后生效，不会热切换 OAuth provider、审批 worker 或 ACL，也不会中断当前在线会话。

页面只返回 `app_secret_set`、`verification_token_set`、`encrypt_key_set` 和 `token_set`，不会读取或回显秘密。秘密输入留空表示保持，填写新值表示替换；清除必须勾选对应危险操作并二次确认。聚合 JSON 虽不经 API 暴露，但会进入数据库备份，因此备份文件必须按敏感凭证保护。

飞书回调必须是合法 HTTPS URL，不得包含用户名、密码、query 或 fragment，路径必须严格为 `/api/v1/auth/feishu/callback`。审批依赖已启用的飞书登录；Verification Token 与 Encrypt Key 至少 16 字符，外部选项 Token 至少 32 字符。只要仍有 `approval_required` 用户，后台就拒绝关闭审批，管理员需先按后续迁移流程处理这些用户，避免取消服务端 ACL 后意外放权。

配置覆盖边界如下：

- 页面可维护：VPN/混淆/MTU/DNS/LAN 路由、通知、飞书登录/审批/外部选项。
- 仅部署层：HTTP 监听、数据库 URL、HTTPS/ACME 域名、数据目录、审计保留期、日志级别、UDP 混淆 PSK、JWT 与 WireGuard 私钥。
- 飞书集成 GET/PUT 只提供管理员访问；秘密不进入响应、日志或审计事件。

## 网络参数

客户端支持[按所在网络排除 VPN 路由](lan-direct-routing.md)。管理员可在“网络设置 → 按网络排除 VPN 路由”配置“客户端所在网段 → 不接管的目标网段”；链路已连接的物理 IPv4 地址匹配时，目标的 TCP、UDP、ICMP 等流量使用系统现有路由，包括原默认网关路径。其余允许目标直接走 VPN。规则通过注册和心跳统一下发，无需修改客户端配置；VPN 虚拟子网始终保留。排除只影响本产品添加的路由，实际路径也可能受其他代理影响。

客户端约每 5 秒检查本机物理接口地址与链路状态，不向目标服务或默认网关发送探测，也不逐连接选择路径或使用透明 TCP 代理。离开匹配网络或规则变更时刷新路由；长连接可能需要重新建立。匹配排除规则的目标若实际不可达，不自动切回 VPN，管理员应确认对应系统路径可用。规则默认空，保存后在客户端下次心跳同步（通常 30 秒内），重连可立即获取；删除或清空规则后恢复原 VPN 路由。

管理员可在“网络设置”页面维护基础 VPN、UDP 混淆、LAN 路由、内置 DNS 和隧道 MTU。基础 VPN 与混淆字段保存为待重启值，服务不会自动重启或强制断线；LAN 路由和 DNS 转发规则立即热更新，MTU 与客户端 DNS 对之后的新连接或重连生效。已有任何 Peer 记录（包括已删除记录）时禁止改变虚拟子网；空节点库保存新子网后会暂停节点注册，直至服务端重启并启用新地址池。启动时若 Peer IP 不属于保存的子网也会拒绝启动。`10.0.0.0/8` 等覆盖 VPN 子网的宽泛 LAN/组路由仍然允许。

客户端 DNS 仅提供“关闭”和“全局”。启用全局后，将 VPN 网关设为系统默认 DNS；这不改变 VPN 数据流量路由，也不接管应用自行配置的 DoH，不覆盖其他 VPN 或系统已有的更具体 DNS 策略。服务端按域名选择上游、静态 A 记录仍可独立配置，不需要客户端域名列表。客户端断开时仅恢复本产品设置，应用失败会回滚。

升级注意：旧数据库、API 请求或客户端响应中的 `split` 模式兼容读取为 `global`，旧 `split_domains` / `domains` 字段忽略，不再输出。旧 `VPN_DNS_MODE=split` 同样表示全局，`VPN_DNS_SPLIT_DOMAINS` 已停止读取。原来只有部分域名经 VPN 解析的配置，升级并重连后会将 VPN 网关设为系统默认 DNS，不再仅限于原分流域名（上述 DoH 与其他 DNS 策略例外仍适用）；如不希望此行为，请在后台将 DNS 关闭。客户端策略仅在新连接或重连时应用，不强制刷新在线连接。

内置 DNS 只绑定 VPN 网关地址的 UDP/TCP 53，不应在 Docker `ports` 或主机防火墙中发布公网 53。它只接受 VPN 子网来源，静态 A 记录优先，转发规则按最长域名后缀选择上游，其余使用默认上游，并支持上游故障切换、UDP 截断后的 TCP 回退和有界缓存。Linux 客户端需要 systemd-resolved 的 `resolvectl`；客户端不会修改 `/etc/resolv.conf` 或物理网卡 DNS。

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
- 接口名首次由 `VPN_WG_INTERFACE`（默认 `wg0`）初始化，之后以“网络设置”保存值为准。
- 审批授权首期只支持显式 `kernel`。`userspace`/`auto` 不启用审批 ACL；不要在这些后端配置飞书审批事件环境变量。
- ACL 使用独立 `inet yilian_vpn_acl` table，只处理 `iifname=wg0` 的 FORWARD 流量，不修改 INPUT、OUTPUT 或 NAT。授权以短租约刷新；服务异常后租约自动到期并停止业务访问。

## 端口

| 端口 | 协议 | 用途 |
|---|---|---|
| `VPN_BIND_ADDR` 端口（默认 8080） | TCP | HTTP API + Web 后台 |
| 80 / 443 | TCP | 建议由外部反向代理提供 HTTPS Web/API；`vpn-server` 当前不直接监听这两个端口。 |
| `VPN_OBFS_BIND_ADDR`（默认 47358） | UDP | 唯一公网混淆数据平面；51820 仅容器内部使用 |
| VPN 网关地址的 53 | UDP + TCP | 内置 DNS，仅隧道内访问，**不要发布到公网** |

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

## 客户端版本与本地安装包镜像

管理员在「客户端版本」页面填写客户端可访问的服务端地址（例如
`https://vpn.xe-flow.com:8443`），保存后点击「立即同步最新版本」。
开启自动同步后，一分钟内首次检查，此后每小时检查 GitHub `DreamAgo/vpn`
的最新正式 Release。默认关闭；设置及最后同步结果保存到
`VPN_DATA_DIR/client-update-sync.json`，重启后仍然有效。

同步在服务端后台执行，关闭浏览器不会中断。服务端需要访问 GitHub API、GitHub
及其 release-assets 下载域名，客户端只需访问本服务端：

- 更新清单：`/updates/latest.json`
- 本地安装包及签名：`/updates/releases/<发布批次>/<文件名>`
- 包括 Windows 安装器、macOS DMG/更新归档、Linux AppImage/deb。

所有包先下载到数据目录内的私有临时目录，校验文件长度和 GitHub SHA-256，
并核对更新签名文件与清单中的签名一致，然后发布文件并原子替换清单。
下载失败、缺包、摘要错误均保留旧清单。客户端仍使用原 Tauri 公钥校验更新包签名。
相同版本的完整本地镜像会复用；低于已发布版本的 GitHub Release 会被拒绝。
单文件限制 1 GiB、单次发布总量限制 4 GiB、同步超时 30 分钟。

旧批次保留，以保证已经取得旧清单的客户端仍可下载；管理员可在确认不再使用后
清理旧批次。异常关机遗留的 `.client-update-staging-*` 临时目录也可在无同步任务时清理。
数据目录应持久化并留足磁盘空间。安装包与镜像设置为文件数据，需随数据目录备份。

管理接口均要求管理员 JWT：

- `GET /api/v1/admin/client-updates`：当前版本、文件下载地址、同步结果与状态。
- `PUT /api/v1/admin/client-updates`：保存 `auto_sync` 和 `public_base_url`。
- `POST /api/v1/admin/client-updates/sync`：启动后台同步；轮询 GET 查看结果。

客户端登录地址和更新地址是独立配置。已发布的 0.1.21 仍使用旧 IP 更新地址；
只有将旧入口同步或转发到新清单，或安装使用新更新地址的客户端，才能发现此镜像。
