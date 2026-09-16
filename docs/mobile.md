# Android 客户端（易链）

Android 首版使用 Kotlin 原生界面、`VpnService` 和共享 Rust `vpn-mobile` 数据面；iOS 尚未实现。最低 Android 8/API 26，目标 API 35，支持 ARM64、ARMv7 和 x86_64。无需 root。

开发基线已同步到 `v0.1.33`。APK 版本、界面版本和注册节点时上报的客户端版本统一读取根 `Cargo.toml` 的 `workspace.package.version`。Android `versionCode` 按 `major × 1000000 + minor × 1000 + patch` 生成（minor/patch 小于 1000）；`0.1.33` 对应 `1033`，后续发布只需更新项目统一版本。

## 本地构建

工具版本：JDK 17、Gradle Wrapper 8.11.1、AGP 8.9.2、Kotlin 2.1.20、Rust 1.90、cargo-ndk 4.1.2、Android SDK 35、NDK 28.2.13676358。

本机已配置工具环境时执行：

```sh
source "$HOME/Library/Android/env.sh"
mobile/scripts/build-android.sh assembleDebug
mobile/android/gradlew -p mobile/android testDebugUnitTest lintDebug
cargo test --locked -p vpn-mobile -p vpn-obfs
cargo clippy --locked -p vpn-mobile --all-targets -- -D warnings
```

其它开发机安装同版本工具，并设置 `JAVA_HOME`、`ANDROID_HOME`。安装 Rust targets：

```sh
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
cargo install cargo-ndk --version 4.1.2 --locked
sdkmanager 'platforms;android-35' 'build-tools;35.0.0' 'ndk;28.2.13676358'
```

Gradle `preBuild` 自动调用 `mobile/scripts/build-native.sh` 编译真实 Rust 源码，生成三个 ABI 的 JNI 库。输出 APK：`mobile/android/app/build/outputs/apk/debug/app-debug.apk`。安装到已授权设备：

```sh
adb install -r mobile/android/app/build/outputs/apk/debug/app-debug.apk
```

调试包使用 Android 自动生成的调试签名；不要作为生产发布签名。签名/对齐检查：

```sh
"$ANDROID_HOME/build-tools/35.0.0/apksigner" verify --verbose mobile/android/app/build/outputs/apk/debug/app-debug.apk
"$ANDROID_HOME/build-tools/35.0.0/zipalign" -c -P 16 4 mobile/android/app/build/outputs/apk/debug/app-debug.apk
```

## 使用与行为

输入 HTTPS 服务地址、账号和密码，登录后授权系统 VPN，再连接。强制改密账号先完成改密，随后重新登录（服务端会撤销旧会话）。状态在真实 WireGuard 握手完成后显示“已连接”，同时显示 VPN IP 和数据面流量。未在 20 秒内完成握手将退避重连。

后台前台服务通知可直接断开；主动断开、系统撤销 VPN 权限会关闭 TUN、UDP、HTTPS 请求和原生句柄。网络切换将重新注册本设备并建立隧道，间隔从 1 秒退避到 30 秒。每 30 秒使用本机公钥心跳；认证过期串行刷新一次，会话撤销停止隧道。锁屏后台依赖系统前台 VPN 服务，不申请无限唤醒锁。OEM 省电策略需真机检验。

支持原生 WireGuard 与 `obfs-v1` 的 `low-overhead-v1`、`paranoid-v1` 模式；私钥在本机生成，密码不落盘，私钥和令牌以 Android Keystore AES-GCM 加密后存储，不参与备份。HTTPS 保留证书与主机名验证，拒绝明文与重定向。HTTPS 和外层 UDP 均绑定物理 `Network`，且 UDP 调用 `VpnService.protect`，防止流量重新进入隧道。

服务端端点首版使用 IPv4（域名解析筛选 IPv4），与混淆路径 MTU 的 IPv4 开销保持一致；IPv6 互联网流量通过底层网络直连。路由沿用项目的 IPv4 分隧道约定，拒绝默认路由/IPv6 路由，保留 VPN 子网。匹配当前物理接口地址的 `local_route_bypass` 规则会从授权路由减去指定网段，最多 1024 条有效路由；心跳热更新原子替换 TUN 路由。旧服务端空心跳和省略绕行策略保持兼容；显式空绕行列表清除策略。DNS global 模式使用 VPN 子网网关，disabled/缺省不配置 VPN DNS。

## GitHub Actions

`.github/workflows/mobile.yml` 对移动代码及依赖变化的 PR 编译；Actions → Android → Run workflow 可手动构建。每次运行保存 `android-debug` APK，并运行 Rust 数据面测试、Kotlin 控制面/生命周期单元测试和 Android lint。此实现只添加工作流，本次未推送或触发远程运行。

默认无需生产签名。手动勾选 `release` 后需要以下仓库 Secrets，才生成并保存 `android-release` APK/AAB：

| Secret | 内容 |
| --- | --- |
| `ANDROID_KEYSTORE_BASE64` | JKS 文件的 Base64 内容 |
| `ANDROID_KEYSTORE_PASSWORD` | Keystore 密码 |
| `ANDROID_KEY_ALIAS` | 签名 key alias |
| `ANDROID_KEY_PASSWORD` | 签名 key 密码 |

本机发布可设置 `ANDROID_KEYSTORE_PATH`、`ANDROID_KEYSTORE_PASSWORD`、`ANDROID_KEY_ALIAS`、`ANDROID_KEY_PASSWORD` 后执行 `./gradlew assembleRelease bundleRelease`。工作流不发布商店；生产密钥不提交版本控制。

## 验证记录（2026-09-16）

本地 `assembleDebug`、15 项 Kotlin/JVM 测试、17 项 Rust 测试与 clippy 均通过；Android lint 无错误（保留目标 SDK/中文文本的非阻断提示）。最终调试 APK 的 v2 签名、16 KiB ZIP 对齐检查通过，三架构 JNI 库已打包；ARM64 ELF LOAD 段按 16 KiB 对齐。

Rust 自动化覆盖真实客户端与 boringtun 服务端握手、原生及两种混淆模式的双向 IP 报文（含非 16 字节对齐长度）、重放丢弃、MTU 超限、DNS 校验、策略排除和路由数界限。Kotlin 主机测试覆盖刷新一次、撤销清凭据、改密清会话、取消刷新不落盘、断开后的所有权和并发启动。

本次未连接安卓设备，以下仍待真机验证，不能由编译或主机测试替代：

- 有效账号登录、强制改密、系统 VPN 权限拒绝/同意、后台显示独立手机节点。
- 原生和两种混淆模式下访问授权内网、DNS 查询与普通互联网分流。
- Wi-Fi/蜂窝切换、锁屏及 OEM 后台限制、服务端路由和绕行规则热更新。
- 会话撤销停止隧道、通知断开、系统撤销权限、快速断开后重连。
- API 26 与 Android 15/16 KiB 页大小设备的安装和运行。

JNI 内存契约：Java 输入复制后使用，返回数组由 JVM 持有；句柄是注册表 ID，不是地址，`process`/`destroy` 通过锁串行化，过期句柄安全失败。Rust 不持有平台资源、不创建线程；Android 关闭 IO 后销毁句柄。
