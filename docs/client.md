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

## 工作原理

1. `login` 调用服务端认证，拿到 access/refresh token，凭证存入系统凭据库（`CredentialStore`）。
2. 连接时客户端本地生成 WireGuard 密钥对，向服务端 `POST /peers/register` 注册公钥，得到分配的**静态 VPN IP** 与服务端公钥/endpoint。
3. daemon 打开 TUN 设备、配置 IP，建立隧道，并每 30 秒发送心跳。
4. 连接中断或网络切换时按指数退避自动重连。

## 凭据与隐私

- Refresh Token 存系统钥匙串（macOS Keychain / Linux libsecret / Windows Credential Manager）；无钥匙串服务时降级为加密文件（XSalsa20Poly1305）。
- Access Token 仅驻留内存。

## 权限说明

创建 TUN 设备需要管理员权限：

- Linux：`CAP_NET_ADMIN` 或 root。
- macOS：管理员权限创建 `utun`。
- Windows：管理员 + WinTun 驱动。

> 真实隧道转发与各平台系统集成的当前状态见 [REAL-HARDWARE-CHECKLIST.md](REAL-HARDWARE-CHECKLIST.md)。Web 后台「接入指南」页也提供 `vpn.conf` 下载，可临时用官方 WireGuard 客户端导入。
