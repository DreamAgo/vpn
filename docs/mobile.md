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

## 手机端界面

「我的」页底部和「关于易链」显示界面源码构建标识，用于区分版本号相同的本地调试包；此标识不改变统一版本或更新比较规则。

Android 原生界面按已确认的蓝色手机 Demo 实现，主色 `#2563EB`，浅灰背景、白色卡片与底部三页导航，沿用易链原图标。

- **登录**：企业 HTTPS 服务地址、飞书主入口与密码登录；不显示 Demo 场景或模拟账号。
- **连接**：固定一屏，不使用滚动容器；顶部直接显示工作网络卡片，不再显示易链品牌标题栏。中部连接圆环根据可用高度收缩，主操作和底部导航固定可见。详情统一从顶部工作网络卡片进入；底部 Tab 无边框、无选中色块，仅以图标和文字颜色表示当前页。空闲说明、已连接指标与错误提示按状态切换，详细信息进入弹层；小高度窗口将指标收至详情，不堆叠内容。注册、重连、断开时不把旧流量当成当前连接。
- **活动**：真实近期诊断事件，支持查看、复制及清空；日志仍只驻留进程内存。
- **我的**：真实账号、服务地址、改密、应用更新、诊断、系统 VPN 设置、关于与退出。
- 改密、切换服务、退出和安装更新仍协调断开；下载签名/摘要校验与系统 VPN/安装授权逻辑保持生效。
- 错误展示真实脱敏说明。`6001` 不自动解释为终端配额；原型中的配额场景仅用于演示。

## 桌面功能对齐

| 能力 | Android 行为 |
| --- | --- |
| 密码与飞书登录 | 独立登录页提供两种方式；飞书未配置时仍可密码登录，授权使用系统浏览器并固定 `accounts.feishu.cn` HTTPS 主机 |
| 授权取消与生命周期 | 最长 5 分钟轮询，支持取消；页面销毁/旋转终止当前操作，重新发起登录，不保存迟到会话；已收到但未提交的会话尽力撤销 |
| 更新 | 启动已有账号或登录成功自动检查，也可手动检查；连接期间可检查、确认下载并显示进度；安装先协调断开 |
| 账号 | 显示真实用户名；改密确认两次且至少 8 位字母数字，成功后重新登录；连接中账号只读 |
| 连接详情 | IP、流量、持续时间、最近握手、DNS、路由；可复制详情 |
| 诊断 | 最近 120 条、单条最多 240 字符的内存日志，可查看、刷新、清空、复制；错误不输出服务端原始文本或凭据 |
| 系统集成 | 前台 VPN 通知、系统 VPN 设置入口；不支持桌面托盘、开机驻留或 Android 始终开启 VPN |

服务器最低版本/权限拒绝（2002）停止重试并保留账号，提示到「我的 → 应用更新」检查。复制诊断包含内网地址，请按需要分享；日志不持久化，进程退出后清空。

### APK 更新分发与安装

Android 从配置服务器的 `/updates/latest.json` 读取 `downloads`，选择唯一的 `vpn-android-universal-0.1.34.apk`（示例版本，无 `v` 前缀）。下载 URL 必须是同一 HTTPS 源的 `/updates/releases/<UUID>/<标准文件名>`，不接受重定向、用户信息、查询或片段。大小上限 256 MiB，完整检查大小、SHA-256、应用包名、版本名称/递增版本号及当前 APK 签名集合。

服务端镜像将 GitHub 正式发布中可选的标准 APK 纳入本地 `downloads`，校验 GitHub SHA-256 后才发布清单；不改变 Tauri `platforms` 或现有桌面 `.sig` 校验。没有 APK 的旧发布继续支持桌面，Android 明示不可用；AAB 不镜像也不直接安装。将正式签名 APK 附加 GitHub Release 属于单独发布操作，本次未执行。

每次下载使用独立的临时文件和已验证 APK 文件，页面重建后的清理不会影响另一下载。安装通过仅授权读取单个已验证 APK 的私有内容提供器交给系统安装器；首次需要系统“允许此来源安装应用”权限，拒绝后仍可使用原应用。当前采用签名集合严格相等，不接受签名轮换；调试签名与正式签名不同，不能直接覆盖升级。实际发布须持续使用同一生产签名。安装前会重新校验文件与当前服务器，取消安装不会注销账号。

## GitHub Actions

`.github/workflows/mobile.yml` 对移动代码及依赖变化的 PR 编译；Actions → Android → Run workflow 可手动构建。每次运行保存 `android-debug` APK，并运行 Rust 数据面测试、Kotlin 控制面/生命周期单元测试和 Android lint。此实现只添加工作流，本次未推送或触发远程运行。

默认无需生产签名。手动勾选 `release` 后需要以下仓库 Secrets，才生成并保存 `android-release` APK/AAB：

| Secret | 内容 |
| --- | --- |
| `ANDROID_KEYSTORE_BASE64` | JKS 文件的 Base64 内容 |
| `ANDROID_KEYSTORE_PASSWORD` | Keystore 密码 |
| `ANDROID_KEY_ALIAS` | 签名 key alias |
| `ANDROID_KEY_PASSWORD` | 签名 key 密码 |

本机发布可设置 `ANDROID_KEYSTORE_PATH`、`ANDROID_KEYSTORE_PASSWORD`、`ANDROID_KEY_ALIAS`、`ANDROID_KEY_PASSWORD` 后执行 `./gradlew assembleRelease bundleRelease`。签名构建产物标准名为 `vpn-android-universal-<version>.apk` / `.aab`，供后续审核发布。工作流不发布 GitHub Release 或商店；生产密钥不提交版本控制。

## 验证记录（2026-09-16）

本地 `assembleDebug`、29 项 Kotlin/JVM 测试、17 项 Rust 测试与 clippy 均通过；Android lint 无错误（8 项目标 SDK、API 弃用和文本等非阻断提示）。最终调试 APK 的 v2 签名、16 KiB ZIP 对齐检查通过，三架构 JNI 库已打包；ARM64 ELF LOAD 段按 16 KiB 对齐。

Rust 自动化覆盖真实客户端与 boringtun 服务端握手、原生及两种混淆模式的双向 IP 报文（含非 16 字节对齐长度）、重放丢弃、MTU 超限、DNS 校验、策略排除和路由数界限。Kotlin 主机测试覆盖刷新一次、撤销清凭据、改密清会话、取消刷新不落盘、断开后的所有权和并发启动。

本次未连接安卓设备，以下仍待真机验证，不能由编译或主机测试替代：

- 飞书浏览器授权返回、取消/超时、旋转重建、真实账号显示。
- 相同签名高版本 APK 下载、未知来源权限拒绝/同意、系统安装取消/完成；异签名 APK 拒绝。
- 连接时切换连接、活动、我的三个页面、诊断复制、更新检查，安装/退出前断开协调。
- 有效账号登录、强制改密、系统 VPN 权限拒绝/同意、后台显示独立手机节点。
- 原生和两种混淆模式下访问授权内网、DNS 查询与普通互联网分流。
- Wi-Fi/蜂窝切换、锁屏及 OEM 后台限制、服务端路由和绕行规则热更新。
- 会话撤销停止隧道、通知断开、系统撤销权限、快速断开后重连。
- API 26 与 Android 15/16 KiB 页大小设备的安装和运行。

JNI 内存契约：Java 输入复制后使用，返回数组由 JVM 持有；句柄是注册表 ID，不是地址，`process`/`destroy` 通过锁串行化，过期句柄安全失败。Rust 不持有平台资源、不创建线程；Android 关闭 IO 后销毁句柄。

### 6001 校验错误排障

`6001` 是服务端参数校验错误，不能仅凭错误码确定具体原因。Android 展示脱敏后的服务端说明（例如终端配额已满、配置等待重启或飞书资料校验失败）；连接遇到该错误时停止自动重试并保留登录。运行日志仅记录错误码，不写入任意服务端原始消息。排障时需结合出错操作和界面完整提示。

### 蓝色手机界面验证（2026-09-16）

按确认的蓝色 Demo 实现原生三页导航、独立登录、真实活动时间线和二级弹层。已通过 35 项 Kotlin/JVM 测试（新增 6 项连接展示边界），`assembleDebug` 与 `lintDebug`（0 错误、9 项非阻断警告）。Android 8 使用深色系统导航栏，API 27 起使用浅色导航栏，避免低版本属性不兼容。

复核修复了更新操作时无法断开、未登录缺少诊断入口、更新弹层状态与按钮不一致等问题。未连接 Android 设备，未执行安装后视觉检查、字体放大、键盘、系统授权回跳和真实 VPN 联调；这些仍需真机验证。界面不包含模拟账号、流量、版本或错误原因。
