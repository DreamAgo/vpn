# Windows 运行时依赖:wintun.dll

桌面端在 Windows 上的数据面(`vpn-cli` 的用户态 WireGuard)通过 `tun` crate 使用
[Wintun](https://www.wintun.net/) 虚拟网卡驱动。Wintun 以一个**签名的 DLL** 形式分发,
程序运行时加载它来创建虚拟网卡。

本目录用于放置随安装包分发的 `wintun.dll`:

- `wintun.dll` **不入库**(见 `.gitignore`)——它是 WireGuard LLC 官方签名的第三方二进制,
  且按 CPU 架构区分(amd64 / arm64 / x86)。
- **Windows 构建前**先获取它:在仓库根或 `desktop/` 下运行
  ```powershell
  pwsh desktop/scripts/fetch-wintun.ps1            # 默认 amd64
  pwsh desktop/scripts/fetch-wintun.ps1 -Arch arm64
  ```
  脚本会从官方下载并把对应架构的 `wintun.dll` 解压到本目录。

## 打包与加载约定

- `tauri.windows.conf.json` 仅在 **Windows 构建**时合并,把 `resources/wintun.dll`
  作为资源打进安装包(落到资源目录 = exe 同级)。macOS / Linux 构建不引用本文件,
  因此**不需要**这个 DLL,也不会因缺失而构建失败。
- 运行时:`lib.rs` 的 `setup()` 在 Windows 上把该 DLL 的绝对路径写入环境变量
  `VPN_WINTUN_PATH`;`vpn-cli` 的 `wg_userspace.rs` 据此用 `wintun_file()` 显式 load,
  不依赖工作目录搜索。未设置(如 dev 未打包)时回退 `tun` 默认的 `wintun.dll` 搜索。

## 许可与签名

- 仅使用 wintun.net 的**官方签名**版本;自行重新编译的 DLL 未签名,Windows 无法加载其驱动。
- 分发前确认当前 Wintun 的许可条款允许随应用一起分发(官方预编译二进制允许由
  Wintun 库的使用者再分发)。
