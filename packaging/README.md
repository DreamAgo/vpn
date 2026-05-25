# 打包脚手架（packaging/）

本目录为 VPN 项目的跨平台安装包**脚手架**：提供可被 CI / 真机直接使用的打包
配置与脚本。开发机为 macOS，无法实际构建/验证全部安装包，因此本目录的产物
**需在对应平台的 CI 或真机上构建验证**。本目录只包含打包资产，不修改任何
crate / 前端 / migrations。

## 发布二进制

先在仓库根构建：

```sh
cargo build --release --workspace
# 产物：target/release/vpn-server、target/release/vpn-cli
```

| 二进制       | 角色   | 主要平台   | 备注                                   |
| ------------ | ------ | ---------- | -------------------------------------- |
| `vpn-server` | 服务端 | Linux      | 需 `CAP_NET_ADMIN`，system 级 systemd  |
| `vpn-cli`    | 客户端 | 三平台     | 含 `daemon` 子命令，注册用户级服务      |

`vpn-cli daemon install` 由 `vpn-platform` 的 `DaemonRuntime` 实现：
Linux → systemd **user** service；macOS → launchd LaunchAgent
(`com.vpn-cli.daemon`)；Windows → Windows Service (`sc.exe`)。注册时使用参数
`daemon run`。安装包附带的服务单元仅作为参考模板 / 备选注册方式。

---

## 6.3 Linux（`linux/`，.deb + .rpm，nfpm）

打包**服务端 + 客户端**。服务端以 system 级 systemd 服务运行。

- 前置工具：[nfpm](https://nfpm.goreleaser.com/install/)。
- 构建（从仓库根）：

  ```sh
  cargo build --release --workspace
  nfpm package -f packaging/linux/nfpm.yaml -p deb -t dist/
  nfpm package -f packaging/linux/nfpm.yaml -p rpm -t dist/
  ```

- 安装内容：
  - `/usr/bin/vpn-server`、`/usr/bin/vpn-cli`
  - `/lib/systemd/system/vpn-server.service`（system 服务，`CAP_NET_ADMIN`，
    `After=network-online.target`，`EnvironmentFile=/etc/vpn-server/vpn-server.env`）
  - `/etc/vpn-server/vpn-server.env`（config 文件，升级不覆盖）
  - `/var/lib/vpn-server/`（数据目录，归 `vpn-server` 用户）
- 脚本：
  - `scripts/postinstall.sh`：创建 `vpn-server` 系统用户、修权限、`systemctl daemon-reload` + `enable`。
  - `scripts/preremove.sh`：`stop` + `disable` 服务（不删数据/配置）。
- 安装后配置与启动：

  ```sh
  sudo vi /etc/vpn-server/vpn-server.env   # 设置 VPN_DOMAIN / VPN_ENDPOINT 等
  sudo systemctl start vpn-server
  sudo systemctl status vpn-server
  ```

- 环境变量见 `linux/vpn-server.env.example`：`VPN_BIND_ADDR`、`DATABASE_URL`、
  `VPN_HTTPS`、`VPN_DOMAIN`、`VPN_DATA_DIR`、`VPN_SUBNET`、`VPN_LISTEN_PORT`、
  `VPN_ENDPOINT`、`VPN_AUDIT_RETENTION_DAYS`。
- 需真机/CI：`nfpm package` 需在 Linux 上运行；arm64 包需对应 target 交叉构建。

---

## 6.4 macOS（`macos/`，.pkg）

打包**客户端 vpn-cli**，安装到 `/usr/local/bin`。

- 前置工具：Xcode Command Line Tools（`pkgbuild` / `productbuild`）。
- 构建（macOS 真机/CI）：

  ```sh
  cargo build --release --workspace
  sh packaging/macos/build-pkg.sh 0.1.0   # 产物 dist/vpn-cli-0.1.0.pkg
  ```

  通用二进制（Intel + Apple Silicon）需分别构建两个 target 后用 `lipo -create` 合并。
- 文件：
  - `build-pkg.sh`：用 `pkgbuild` + `productbuild` 生成 .pkg。
  - `scripts/postinstall`：修可执行权限并提示注册 daemon。
  - `com.vpn-cli.daemon.plist`：LaunchAgent 模板（label / 参数与 `DaemonRuntime` 一致）。
- daemon 注册：推荐用户登录后执行 `vpn-cli daemon install`（LaunchAgent 属于
  登录会话，root 安装上下文无法可靠 load，故 postinstall 不自动 load）。
- 需真机/签名证书：
  - `.pkg` 分发前需用 **Developer ID** 证书 `productsign` 签名，并经
    `notarytool` 公证 + `stapler staple`，否则 Gatekeeper 会拦截。
  - 相关命令在 `build-pkg.sh` 中以注释占位，需 Apple 开发者账号 + 证书。

---

## 6.5 Windows（`windows/`，Inno Setup）

打包**客户端 vpn-cli.exe**，安装到 `Program Files\VPN CLI`。

- 前置工具：[Inno Setup 6+](https://jrsoftware.org/isdl.php)（`ISCC.exe`）。
- 构建（Windows 真机/CI）：

  ```powershell
  cargo build --release --workspace
  pwsh packaging\windows\build-installer.ps1 -Version 0.1.0
  # 产物 dist\vpn-cli-setup-0.1.0.exe
  ```

- 文件：
  - `vpn-cli.iss`：Inno Setup 脚本。安装 exe、可选加入系统 PATH、可选注册
    Windows 服务（`vpn-cli daemon install`），卸载时 `daemon uninstall`。
  - `build-installer.ps1`：定位并调用 `ISCC.exe` 编译。
- 需管理员权限（PATH 修改 / 服务注册），`.iss` 已设 `PrivilegesRequired=admin`。
- 需真机/签名证书：分发前建议用代码签名证书 `signtool` 签名安装器（占位注释
  在 `build-installer.ps1`）。`ISCC` 编译只能在 Windows 上进行。

---

## 自检与局限

- Shell 脚本（`*.sh`、macOS `postinstall`）均通过 `sh -n` 语法自检。
- YAML（`nfpm.yaml`）通过通用 YAML 合法性自检。
- 未做、需真机/CI 的部分：实际 `nfpm` / `pkgbuild` / `ISCC` 构建、代码签名与
  公证、systemd/launchd/服务管理器交互、跨架构二进制产出。
- 版本号当前固定为 `0.1.0`（与 workspace 一致），CI 可经参数 / `/D` 宏注入实际 tag。
