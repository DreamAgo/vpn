# VPN Client — Desktop GUI (Tauri 2)

A cross-platform menu-bar (tray) VPN client. **单进程架构**:GUI 直接把 `vpn-cli`
当库调用,在**本进程内**完成整条用户态 WireGuard 隧道(boringtun + TUN + 路由 +
心跳)。**不需要**单独启动 daemon,也没有 IPC / unix socket。

## Architecture

```
┌────────────────────┐      Tauri commands       ┌──────────────────────────┐
│  React popover UI   │ ────────────────────────▶ │  src-tauri (Rust)        │
│  (desktop/src)      │ ◀──────────────────────── │  commands.rs → manager   │
└────────────────────┘   get_status / connect /   └───────────┬──────────────┘
                         disconnect / login / ...              │ 进程内库调用 vpn-cli:
                                                               │  daemon::connect_once
                                                               │  daemon::bring_up_tunnel  ← 用户态 boringtun 隧道
                                                               │  daemon::run_heartbeat
                                                               │  daemon::SharedState (状态)
                                                               ▼
                                              本进程直接开 TUN + 加路由 + 转发(需 root/管理员)
```

`manager.rs` 里的 `VpnManager`(放进 Tauri 托管状态,以 `Arc` 共享)持有:本机
WireGuard 密钥对、`SharedState`(连接状态)、当前连接的关停信号。`connect()` 跑
`connect_once`(注册)→ `bring_up_tunnel`(开 TUN + boringtun 转发循环)→ spawn
`run_heartbeat`;`disconnect()` 发关停信号(转发任务自行删路由/关设备)。

Commands(`#[tauri::command]`):
- `get_status() -> StatusResponse` — 读本进程状态,不会失败。
- `connect() / disconnect() -> Result<(), String>`
- `login(server, username, password) / logout() -> Result<(), String>`
- `is_logged_in() -> bool`, `saved_server() -> Option<String>`, `hide_window()`

## ⚠️ 必须以特权运行

开 TUN 设备需要 root/管理员,**单进程方案下整个 App 都要特权运行**:
- **macOS**(release):双击图标即弹系统管理员密码框自提权(`maybe_elevate` 经 osascript +
  `launchctl asuser` 以 root 重启自己);取消提权则以非特权模式继续运行(连接会报权限错)。
  dev 构建不自提权,从终端 `sudo cargo tauri dev` 才能真正连接。
- **Windows**(release):由 `requireAdministrator` 应用清单(build.rs 注入)在启动时弹 UAC 自提权;
  需随包分发 `wintun.dll`(见下方「Windows 构建」)。
- **Linux**:暂未实现自提权,需 `sudo` 运行(并确保有 `/dev/net/tun`)。

未以特权运行时,点 Connect 会在面板里报错(TUN 打开失败),不会崩溃。

## Prerequisites

- Node + npm、Rust 1.90+、`tauri-cli` v2(`cargo install tauri-cli --version "^2"`)。
- 先**登录**:在 App 的登录表单填服务端地址 / 用户名 / 密码(凭证存到文件后端
  `~/.config/vpn-cli/creds.enc`)。注意:GUI 以 root 跑时凭证落在 **root 的 home**,
  与普通用户 `vpn-cli login` 的位置不同。

## Develop / Run

```sh
cd desktop
npm install                       # 首次
# 开发(需特权才能真正连接;不特权也能看 UI):
sudo cargo tauri dev              # 从你的终端 sudo,窗口才显示
# 或构建后以特权运行:
cargo tauri build
sudo "src-tauri/target/release/bundle/macos/VPN Client.app/Contents/MacOS/vpn-desktop"
```

App 在菜单栏(无 Dock 图标,`ActivationPolicy::Accessory`)。左键托盘图标切换面板,
失焦自动隐藏;右键托盘 = Open Panel / Connect / Disconnect / Quit。首次启动会直接
弹出面板便于发现 UI。

## Build

```sh
cd desktop
npm run build                     # 仅前端(Vite → dist/)
cargo tauri build                 # 完整打包(.app/.dmg)
cd src-tauri && cargo build       # 仅编译 Rust 侧
```

### Windows 构建

Windows 包需要 WireGuard 官方签名的 `wintun.dll`(运行时由 `tun` crate 加载来开虚拟网卡)。
它**不入库**,构建前先获取:

```powershell
cd desktop
pwsh scripts/fetch-wintun.ps1               # 默认 amd64;arm64 用 -Arch arm64
npm run build
cargo tauri build
```

- `fetch-wintun.ps1` 把 `wintun.dll` 放到 `src-tauri/resources/`;`tauri.windows.conf.json`
  仅在 Windows 构建时把它作为资源打进安装包(落到 exe 同级)。
- 运行时:`lib.rs` 的 `setup()` 把该 DLL 绝对路径写入 `VPN_WINTUN_PATH`,`wg_userspace.rs`
  据此显式 load(详见 `src-tauri/resources/README.md`)。
- 提权由 `requireAdministrator` 清单负责,**无需**手动「以管理员身份运行」。

## Notes

- **跨平台**:同一套代码 macOS(utun)/ Linux(`/dev/net/tun`)/ Windows(WinTun)
  都用。Windows 的 `wintun.dll` 打包与提权清单已接线(见「Windows 构建」);Linux 自提权
  尚未实现(需 `sudo` 或后续接 pkexec / 特权 helper)。
- **零外部依赖**:不依赖系统安装的 WireGuard 工具,隧道由内置 boringtun 完成。
- **Standalone workspace**:`src-tauri/Cargo.toml` 声明了自己的空 `[workspace]`,
  **不**属于仓库根 workspace,故重型 Tauri/webview 依赖不影响其它 crate 的
  `cargo build`/clippy/test;经 `path` 依赖 `vpn-cli` + `vpn-wireguard`。
- 图标是占位图(`src-tauri/icons/`);用 `cargo tauri icon path/to/icon.png` 换正式图标。
```
