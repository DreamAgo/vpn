# VPN Client — Desktop GUI (Tauri 2)

A cross-platform menu-bar (tray) VPN client. Windows/Linux 由 GUI 进程直接调用
`vpn-cli`；macOS 使用普通用户 GUI + launchd 常驻的最小 root helper，避免每次
打开应用都要求管理员密码。

## Architecture

```
┌────────────────────┐   Tauri commands   ┌──────────────────────────┐
│  React popover UI   │ ─────────────────▶ │ 普通用户 GUI             │
└────────────────────┘                    └────────────┬─────────────┘
                                                      │ macOS: owner-only UDS
                                                      ▼
                                         ┌──────────────────────────┐
                                         │ launchd root helper      │
                                         │ TUN + 路由 + boringtun   │
                                         └──────────────────────────┘
```

`manager.rs` 里的 `VpnManager` 持有本机
WireGuard 密钥对、`SharedState`(连接状态)、当前连接的关停信号。`connect()` 跑
`connect_once`(注册)→ `bring_up_tunnel`(开 TUN + boringtun 转发循环)→ spawn
`run_heartbeat`;macOS 由 helper 持有该对象，其他平台由 GUI 持有。

Commands(`#[tauri::command]`):
- `get_status() -> StatusResponse` — 读本进程状态,不会失败。
- `connect() / disconnect() -> Result<(), String>`
- `login(server, username, password) / logout() -> Result<(), String>`
- `is_logged_in() -> bool`, `saved_server() -> Option<String>`, `hide_window()`

## 权限模型

开 TUN 设备需要 root/管理员：
- **macOS**：GUI 始终为普通用户。首次连接及 helper 二进制摘要变化时授权一次，
  安装 `/Library/PrivilegedHelperTools/com.xeflow.yilian.helper` LaunchDaemon；日常启动
  不再弹密码。helper socket 为当前控制台用户专用并校验 peer UID，切换用户会先断开隧道。
  安装过程在 root 侧生成固定 plist、校验复制后 SHA-256，并对升级失败统一回滚。
- **Windows**(release):由 `requireAdministrator` 应用清单(build.rs 注入)在启动时弹 UAC 自提权;
  需随包分发 `wintun.dll`(见下方「Windows 构建」)。
- **Linux**:暂未实现自提权,需 `sudo` 运行(并确保有 `/dev/net/tun`)。

macOS 取消首次授权时 GUI 仍可用；不会缓存管理员密码、修改 sudoers 或以 root
运行 WebView。helper 日志在 root-only 轮转目录中，仅经认证 IPC 脱敏返回日志面板。

## Prerequisites

- Node + npm、Rust 1.90+、`tauri-cli` v2(`cargo install tauri-cli --version "^2"`)。
- 先**登录**:在 App 的登录表单填服务端地址 / 用户名 / 密码(凭证存到文件后端
  `~/.config/vpn-cli/creds.enc`)。从旧 root-GUI 版本升级可能需重新登录一次，此后
  凭据始终属于当前用户。

## Develop / Run

```sh
cd desktop
npm install                       # 首次
# 开发：GUI 无需 sudo；首次连接时安装 helper
cargo tauri dev
cargo tauri build
```

macOS 卸载 helper：

```sh
sudo launchctl bootout system/com.xeflow.yilian.helper
sudo rm -f /Library/LaunchDaemons/com.xeflow.yilian.helper.plist \
  /Library/PrivilegedHelperTools/com.xeflow.yilian.helper \
  /var/run/com.xeflow.yilian.helper.sock \
  /var/run/com.xeflow.yilian.helper.install.lock \
  /var/db/com.xeflow.yilian.helper.blocked-tokens
sudo rm -rf /var/log/com.xeflow.yilian.helper
```

App 同时提供 Dock 与菜单栏图标。左键托盘图标切换面板，右键菜单可连接、断开或退出。

## Build

```sh
cd desktop
npm run build                     # 仅前端(Vite → dist/)
cargo tauri build                 # 完整打包(.app/.dmg)
cd src-tauri && cargo build       # 仅编译 Rust 侧
```

GitHub Actions 的 `CI` 工作流会在每次 `main` 推送和 Pull Request 上，用
Windows、macOS、Linux 原生 runner 分别构建前端并执行桌面 Rust 测试与 Clippy。
需要可下载的安装包时，在 GitHub Actions 手动运行 `Release`，或推送 `v*` tag：

- Windows：MSI、NSIS 安装器；
- macOS：Intel 与 Apple Silicon DMG；
- Linux：amd64/arm64 AppImage 与 deb。

手动运行只把结果保留为 Actions artifacts；`v*` tag 才创建 GitHub Release。

## Auto Update

桌面端已接入 Tauri updater。应用启动约 3.5 秒后会静默检查
`src-tauri/tauri.conf.json` 中配置的更新地址（当前为自建服务器的
`/updates/latest.json`），设置面板里也可以手动检查并安装更新。
点击“安装并重启”后下载、校验签名、安装并重启；不会无人确认自动安装。

| 平台 | 架构 | 应用内更新包 |
| --- | --- | --- |
| Windows | x64 | NSIS `.exe` |
| macOS | Intel / Apple Silicon | `.app.tar.gz`（首次安装仍用 DMG） |
| Linux | x64 / ARM64 | `.AppImage` 或 `.deb`，按当前安装格式选择 |

Linux deb 更新使用 updater 2.10.1 的包格式识别，更新清单中的
`linux-<arch>-deb` 优先于通用 AppImage 条目。deb 安装需要 `dpkg` 和系统提权
（例如 `pkexec`）；AppImage 应放在当前用户可写的位置。
macOS 应先把应用从 DMG 拷贝到可安装的位置，再运行应用内升级。

发布流程会收集五个操作系统/架构目标的更新包及签名（Linux 两种格式），
由 `scripts/generate-updater-manifest.py` 生成完整清单。
正式 tag 发布缺少任一更新包或签名时失败，避免发布不完整的更新清单。
手动 workflow_dispatch 无签名密钥时仍可构建普通安装包。

GitHub Release 默认清单中的下载地址指向该 Release。自建更新服务器需要同步
`latest.json`；如果安装包也托管在自建服务器，先同步所有包，再用下面的命令
生成指向镜像的清单，最后替换线上 `latest.json`：

```sh
python3 desktop/scripts/generate-updater-manifest.py \
  --tag v0.1.29 --repository OWNER/REPO \
  --assets-dir release-assets \
  --base-url https://updates.example.com/updates
```

`--tag` 必须与包内应用版本一致；示例域名需要替换为实际包下载地址。
脚本只生成本地清单，不会自动上传或部署。

更新包必须签名:

- 公钥写在 `src-tauri/tauri.conf.json` 的 `plugins.updater.pubkey`。
- 私钥不要提交到仓库,写入 GitHub Secrets:
  - `TAURI_SIGNING_PRIVATE_KEY`
  - `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`(无密码时可留空)
- Release workflow 会用私钥生成 Tauri updater 签名,并生成 `latest.json`。

如果要替换生产密钥:

```sh
cd desktop
npx tauri signer generate --write-keys /secure/path/vpn-desktop-updater.key
```

把输出的 public key 更新到 `tauri.conf.json`,把 private key 写入
`TAURI_SIGNING_PRIVATE_KEY` secret。丢失私钥后,已安装客户端无法信任后续更新,
需要重新下载安装包。

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

## 日志与故障排查

桌面客户端在 Tauri 初始化前启用本地文件日志，默认记录 INFO 级别；设置
`RUST_LOG=debug` 可临时增加细节。为避免 HTTP 依赖泄露认证信息，只接受
`off/error/warn/info/debug/trace` 全局级别且仅启用本项目日志目标。日志按日滚动并保留 7 天，日志系统初始化失败
不会阻止 VPN 启动，设置页“复制诊断信息”会注明日志目录或失败原因。

- Windows：`%LOCALAPPDATA%\vpn-cli\logs\vpn-desktop.log.YYYY-MM-DD`
- macOS：本地应用数据目录下的 `vpn-cli/logs/`
- Linux：`$XDG_DATA_HOME/vpn-cli/logs/`，未设置时通常为
  `~/.local/share/vpn-cli/logs/`

排查“连接一会后退出”时，先复制设置页诊断信息，再收集退出当天及前一天的
`vpn-desktop.log.*`。日志记录启动版本/平台、连接阶段、TUN/路由事件、心跳与
后台任务退出，以及 Rust panic 的线程、位置和回溯。密码、Access/Refresh Token、
WireGuard 私钥和认证头不得写入日志。

也可以在“设置 → 连接 → 查看详细日志”打开应用内日志弹窗。弹窗每 2 秒自动刷新，
支持手动刷新和复制；用户向上滚动阅读时会暂停自动跟随，回到底部后恢复。关闭弹窗
会立即停止日志轮询。每次读取只访问客户端固定日志目录，按时间正序返回近期最多
500 行、256 KiB；超过限制会显示“已截断”。后端不接受任意文件路径，并在返回前
再次对凭证关键词、JWT 和 URL 用户信息做保守脱敏。

默认 INFO 日志会按连接尝试编号依次标出凭证、会话刷新、节点注册、旧任务清理、
endpoint 解析、TUN、UDP、路由、转发任务和心跳阶段，并给出结果与耗时。数据面
“已就绪”仅表示本地 TUN/UDP/路由和转发任务已启动，不代表已经确认 WireGuard 握手。
若弹窗读取失败，最后一次成功快照仍会保留，可点击“重试”；这不会影响正在进行的
VPN 连接。

panic hook 只能捕获 Rust panic；操作系统直接终止、断电，以及 WinTun/WebView2
原生 access violation 可能来不及写出崩溃原因。此时日志末尾仍可用于判断最后成功
阶段，完整原生崩溃转储需另行启用 Windows Error Reporting/minidump。
