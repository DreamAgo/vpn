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
| `vpn-cli daemon uninstall` | 卸载系统服务。 |
| `vpn-cli daemon start` / `stop` / `status` | 控制系统服务。 |
| `vpn-cli daemon run` | 前台运行 daemon 主循环（一般由服务管理器拉起）。 |

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
