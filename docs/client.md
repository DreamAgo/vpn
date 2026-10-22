# 客户端使用（vpn-cli）

`vpn-cli` 是跨平台命令行客户端（macOS / Linux / Windows）。它负责登录、注册节点、建立 WireGuard 隧道，并可作为后台服务常驻。

## 安装

- **从源码**：`cargo build --release --bin vpn-cli`，产物 `target/release/vpn-cli`。
- **安装包**：见 [`packaging/`](../packaging)（macOS `.pkg`、Windows 安装器、Linux 随服务端包附带）。

## 命令

| 命令 | 说明 |
|---|---|
| `vpn-cli login --server <URL> [--username <U>]` | 登录服务端并把凭证存入系统钥匙串。密码交互式安全读入。 |
| `vpn-cli logout` | 注销并清除本地凭证。 |
| `vpn-cli up`（别名 `connect`） | 建立 VPN 连接。 |
| `vpn-cli down`（别名 `disconnect`） | 断开连接。 |
| `vpn-cli status` | 查看当前连接状态（状态 / VPN IP / 流量 / 最近错误）。 |
| `vpn-cli daemon install` | 注册为系统服务（systemd user / launchd / Windows Service）并开机自启。 |
| `vpn-cli daemon install-system` | Linux：安装使用 root 文件凭据的 systemd 系统服务，启动后自动连接。 |
| `vpn-cli daemon uninstall` | 卸载系统服务。 |
| `vpn-cli daemon start` / `stop` / `status` | 控制系统服务。 |
| `vpn-cli daemon run` | 前台运行 daemon 主循环（一般由服务管理器拉起）。 |
| `vpn-cli update install --server <HTTPS URL>` | Linux 系统服务：启用独立的自动升级定时任务，可用 `--schedule` 设置时间。 |
| `vpn-cli update run` / `disable` | 立即检查升级 / 关闭定时升级。 |

## 典型流程

```bash
# 首次：登录（凭证持久化到系统钥匙串）
vpn-cli login --server https://vpn.example.com --username alice
# 首次登录若被要求改密，请先在 Web 后台或按提示修改

# 连接
vpn-cli up
vpn-cli status

# 让它开机自启、后台常驻
vpn-cli daemon install
```

## 桌面端飞书登录

桌面客户端会先探测服务端是否启用飞书登录。启用后点击“使用飞书登录”，客户端在当前图形用户会话中打开系统浏览器，并在三分钟内短轮询授权结果。服务端回调页不包含任何本站或飞书 token；客户端只在一次性领取成功后将原格式的 refresh token 写入现有凭证库。

首次授权优先按邮箱（不区分大小写）绑定已有启用账号；没有匹配时自动创建 `user` 角色、最多 1 台设备、无用户组的账号。新账号不会自动获得业务网段权限。已绑定账号被禁用后，飞书登录同样会被拒绝。

服务端未配置飞书时，入口会明确禁用，账号密码登录不受影响。

## 工作原理

1. `login` 调用服务端认证，拿到 access/refresh token，凭证存入系统凭据库（`CredentialStore`）。
2. 连接时客户端本地生成 WireGuard 密钥对，向服务端 `POST /peers/register` 注册公钥，得到分配的**静态 VPN IP**、服务端公钥和可选 `obfs-v1` 传输配置。
3. daemon 打开 TUN 设备、配置 IP，建立隧道，并每 30 秒发送心跳。
4. 连接中断或网络切换时按指数退避自动重连。

服务端启用混淆后，客户端在同一进程内完成封装，不需要额外代理程序。配置或认证失败时会明确报错，不会回退到可被识别的原生 WireGuard。详见 [UDP 混淆传输](udp-obfuscation.md)。

## 凭据与隐私

- Refresh Token 存系统钥匙串（macOS Keychain / Linux libsecret / Windows Credential Manager）；无钥匙串服务时降级为加密文件（XSalsa20Poly1305）。
- Access Token 仅驻留内存。

## 权限说明

创建 TUN 设备需要管理员权限：

- Linux：`CAP_NET_ADMIN` 或 root。
- macOS CLI：管理员权限创建 `utun`。macOS 桌面端首次连接时授权安装常驻 root
  helper，日常打开不再要求管理员密码；helper 摘要随客户端升级变化时再授权一次。
- Windows：管理员 + WinTun 驱动。

> 真实隧道转发与各平台系统集成的当前状态见 [REAL-HARDWARE-CHECKLIST.md](REAL-HARDWARE-CHECKLIST.md)。Web 后台「接入指南」页也提供 `vpn.conf` 下载，可临时用官方 WireGuard 客户端导入。

### 桌面连接状态通知

桌面界面订阅 `vpn-status-changed` 通知，收到后读取当前状态快照；原有每 2.5 秒轮询继续更新流量并兜底。通知仅针对连接状态、VPN IP、连接起始时间和错误变化，相同状态和流量累计不会触发通知。

Windows/Linux 直接订阅进程内状态；macOS 通过带控制台会话校验的 helper `wait_status` 请求等待变化，每次等待最多 5 秒。变化发生时立即返回，不等待满 5 秒；helper 缺失、版本不支持或暂时不可达时退回轮询，不因订阅状态而触发提权安装。前端监听注册后补读快照，卸载时解除监听，并继续使用请求序号拒绝旧响应。

首次建立隧道时，管理心跳和数据面握手尚未全部就绪会保持“连接中”；已连接后健康检查失败才进入“重连中”。

## Linux 无人值守节点自动升级

支持 Linux amd64 / arm64 的 **root systemd 系统服务**。原来的 `daemon install`
仍安装用户服务，不会自动启用升级。更新任务通过服务端公网 HTTPS 下载，独立于
VPN 隧道、登录 token 和最低版本检查；服务端必须先同步包含 CLI 签名包的新正式 Release。
仅内网可达的更新域名不能在 VPN 断开后提供恢复能力。

首次部署需要人工安装一次支持这些命令的 CLI。推荐放在 `/usr/local/bin/vpn-cli`，
文件和所有父目录须属于 root，其他用户不可写，路径不能包含符号链接。
已安装在包管理器路径的 CLI 建议迁移到此独立路径，避免包管理器覆盖自动升级结果。

```bash
# 从统一正式 Release 获取本机架构的 CLI 后安装。
sudo install -o root -g root -m 0755 ./vpn-cli /usr/local/bin/vpn-cli

# 系统服务固定使用 root 的文件凭据库，不复用普通用户的钥匙串。
sudo env HOME=/root XDG_CONFIG_HOME=/root/.config VPN_CLI_CRED_BACKEND=file \
  /usr/local/bin/vpn-cli login --server https://vpn.example.com --username alice
sudo /usr/local/bin/vpn-cli daemon install-system
sudo systemctl start vpn-cli

# 每小时检查一次，最多随机延迟 5 分钟。
sudo /usr/local/bin/vpn-cli update install --server https://vpn.example.com
# 或限定到每天本机时间凌晨 3 点（离线错过后由 Persistent 补执行）。
sudo /usr/local/bin/vpn-cli update install --server https://vpn.example.com \
  --schedule '*-*-* 03:00:00'
```

从用户服务迁移时，先在原登录用户中执行 `vpn-cli down`，确认状态为 disconnected，
再执行 `vpn-cli daemon uninstall`；然后执行上面的 root 登录、系统服务安装步骤。
不要同时启动用户服务和系统服务。已有自定义系统服务不会被安装命令覆盖，需要先记录其
凭据、环境和网络设置，再迁移到受管理的服务。系统服务启动后自动连接；要长期暂停节点，
使用 `sudo systemctl stop vpn-cli`。

```bash
# 系统服务状态及 VPN 状态（IPC 目录与普通用户服务不同）。
sudo systemctl status vpn-cli
sudo env XDG_RUNTIME_DIR=/run/vpn-cli /usr/local/bin/vpn-cli status

# 立即检查：使用独立更新程序，主程序损坏时此入口仍可恢复未完成的升级。
sudo /usr/local/libexec/vpn-cli-updater update run
sudo systemctl list-timers vpn-cli-update.timer
sudo journalctl -u vpn-cli-update.service -n 100

# 关闭定时升级；不会删除配置和备份。
sudo /usr/local/libexec/vpn-cli-updater update disable
```

更新程序安装到 `/usr/local/libexec/vpn-cli-updater`，不随主程序替换；配置位于
`/etc/vpn-cli-update.json`，只记录服务端地址和可执行文件路径，不保存账号密码。
切换登录服务端时，也需重新执行 `update install --server ...` 切换更新来源。
重新执行安装命令还会刷新独立更新程序；发布包含更新器自身修复或签名公钥轮换时，需通过
运维渠道执行这一步。自动升级主程序不会隐式更换恢复程序。

更新依次检查同源 HTTPS 地址、包大小、内置发布公钥的 Minisign 签名、二进制版本。
拒绝 HTTP、重定向、预发布版本、降级、清单与程序版本不符。全部校验通过后才断开并等待
旧隧道路由清理，原子替换文件，重启此前正在运行的服务；原本停止的服务保持停止。
系统服务每次启动会自动尝试连接，保留 root 文件凭据和账号配置。
初始连接失败后每分钟重试；手动断开和路由清理失败不会触发这种重试。

备份和事务记录保存在 `/var/lib/vpn-cli-update/`。新进程或 IPC 健康检查失败会回滚；
断电或更新进程被中断后，下一次定时任务先完成恢复。回滚失败保留记录继续恢复。
健康检查验证进程稳定和 IPC 可用，不将服务端拒绝或临时网络故障判定为二进制损坏。
回滚过的版本被记录在 `rejected-version`，不会无限重试；下一正式版本可正常升级。
回滚不会绕过服务端最低版本要求。日志通过 journald 查看。

发布顺序：修复先合入 `main` → 统一 Release 从同一标签构建并签名两个 Linux CLI 包
→ 服务端同步完成 → 确认节点版本 → 最后提高最低版本。服务端升级前的同步逻辑不支持
CLI 包，需先部署新服务端。不要把新包补到旧 Release 或移动旧标签。
