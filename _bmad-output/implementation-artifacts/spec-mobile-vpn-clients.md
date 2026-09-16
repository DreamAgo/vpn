---
title: 'Android VPN 客户端及云端构建'
type: 'feature'
created: '2026-09-16'
status: 'done'
baseline_commit: '483e8d5ae28cca49113204d77d49b0ded3965b74'
context: []
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** 手机目前无法使用本项目客户端接入企业 VPN。

**Approach:** 先实现 Android 原生界面和系统 VPN 接口，使用可供未来 iOS 复用的 Rust 协议核心；GitHub Actions 编译、测试并保存构建产物。

## Boundaries & Constraints

**Always:** 兼容现有认证、节点注册、心跳、路由、DNS、MTU 与两种 obfs-v1 模式；本地生成私钥，凭据进入 Android Keystore；HTTPS 校验证书；状态来自真实隧道。Android 最低 API 26。

**Ask First:** 商店发布与生产签名身份由用户提供；签名材料通过 Secrets 配置。

**Never:** 使用 root、桌面 daemon 或伪造连接；首版不含 iOS、飞书登录、站点网关和商店自动发布。

## I/O & Edge-Case Matrix

| 场景 | 输入 | 行为 | 失败处理 |
|---|---|---|---|
| 登录 | 地址、账号、密码 | 注册设备并申请 VPN 权限 | 显示认证错误；强制改密先完成改密 |
| 连接 | 原生或混淆配置 | 握手后显示 IP、流量 | 参数非法或握手超时不得显示已连接 |
| 续期 | Token 过期 | 串行刷新并重试 | 撤销会话后停止隧道并要求登录 |
| 策略变化 | 心跳更新 | 应用路由及本地绕行策略 | 失败停止连接并报错 |
| 生命周期 | 切网、锁屏、断开 | 后台保持、退避重连或清理 | 用户主动断开禁止自动重连 |
| 打包 | 缺少生产签名 | Android 调试 APK | 正式签名缺失不影响调试包构建 |

</frozen-after-approval>

## Code Map

- `crates/vpn-cli/src/api.rs`、`daemon.rs`：认证与会话行为参考。
- `crates/vpn-cli/src/wg_userspace.rs`：数据面参考，含桌面系统依赖。
- `crates/vpn-api-types/src/peer.rs`、`system.rs`：协议与网络策略。
- `crates/vpn-obfs/src/lib.rs`：复用混淆编码。

## Tasks & Acceptance

**Execution:**
- [x] `crates/vpn-mobile/Cargo.toml`、`src/{lib,engine,session,policy,routes,ffi}.rs`、根 `Cargo.toml`：新增共享核心，处理数据包、计时器与策略，认证/刷新由 Android `Api.kt` 绑定物理网络执行；明确跨语言内存所有权及取消语义。
- [x] `mobile/android/`：Gradle 工程、Kotlin 界面、JNI、VpnService；保护外层 socket，配置 TUN、通知、凭据及切网重连。
- [x] `crates/vpn-mobile/tests/`、Android `src/test/`：测试上述异常、报文往返、混淆重放拒绝、MTU 和状态竞态。
- [x] `mobile/scripts/`、`.github/workflows/mobile.yml`：固定工具版本，交叉编译 Rust；PR 编译检查，手动构建下载产物；配置签名后导出发布包。
- [x] `docs/mobile.md`、`README.md`：安装、Secrets、构建步骤及真机验证记录。

**Acceptance Criteria:**
- Given 有效账号和可达服务器，When Android 连接，Then 可访问授权内网，后台可识别独立手机节点。
- Given 隧道运行，When 切网、锁屏或策略改变，Then 按矩阵恢复或明确报错，断开后释放系统资源。
- Given GitHub runner，When 运行移动构建，Then Android 与 Rust 真实源码参与编译，产物及签名状态明确。

## Spec Change Log

- 2026-09-16：用户要求“先实现安卓”，作为实施确认；收敛到 Android，iOS 延期。

## Design Notes

共享核心不依赖整个 vpn-cli；系统网络 IO 由原生层负责，避免引入桌面路由与凭据后端。iOS 按用户最新要求延期。云端编译不能替代真机 VPN 测试。

## Verification

- `cargo test -p vpn-mobile`、`cargo clippy -p vpn-mobile --all-targets -- -D warnings`：核心测试与静态检查通过。
- 本机与 GitHub Linux 构建 Android APK；签名后验证产物。
- Android 真机检查登录、改密、内网通信、DNS、混淆、切网、撤销及断开；未执行项明确保留。
- 本机仅有 Xcode Command Line Tools；Android 工具链现已安装，详见 `docs/android-local-build.md`。工具链验证不等于客户端构建通过。

### 已执行结果（2026-09-16）

- `cargo test --locked -p vpn-mobile`：17 项通过（包括真实 boringtun 与两种混淆的数据包互通、JNI 句柄和路由边界）。
- `cargo clippy --locked -p vpn-mobile --all-targets -- -D warnings`：通过。
- `mobile/android/gradlew -p mobile/android --no-daemon assembleDebug testDebugUnitTest lintDebug`：通过，15 项 JVM 测试零失败，lint 无错误。
- 调试 APK 三 ABI 打包、apksigner v2 签名验证、zipalign 16 KiB 检查通过；ARM64 ELF LOAD 对齐 0x4000。
- 无安卓设备连接：实际服务端登录/内网访问、权限、锁屏、物理切网、OEM 后台运行尚未验证。GitHub 工作流已添加但本次没有推送/远程运行；生产签名未配置。

## Review Results

- 已完成盲审、边界审查与验收审查，确认问题已修复并添加回归测试。
- 修复 VPN 前后网络选择不一致导致反复重连；明确排序、同级保留当前网络。
- 令牌写入和清除统一经过取消提交门禁；账号操作同步 Vault；刷新后退出使用最新令牌。
- 成功握手后重置重连退避，未运行时断开不会卡在断开中。
- 未更改空 allowed_routes 的旧服务端兼容语义：现有服务端始终至少下发 VPN 子网，CLI 同样保留空响应前的策略。
- Lint 为 0 errors / 3 warnings：目标 API 35 与两条文本国际化提示；不将这些提示等同于真机验证。

## Suggested Review Order

- 平台入口与资源释放
  [TunnelService.kt:18](../../mobile/android/app/src/main/java/com/biubiu/vpn/TunnelService.kt#L18)

- 认证续期与取消提交
  [Api.kt:13](../../mobile/android/app/src/main/java/com/biubiu/vpn/Api.kt#L13)

- 稳定的物理网络选择
  [NetworkSelection.kt:5](../../mobile/android/app/src/main/java/com/biubiu/vpn/NetworkSelection.kt#L5)

- 真实协议处理与混淆
  [engine.rs:76](../../crates/vpn-mobile/src/engine.rs#L76)

- JNI 句柄和内存边界
  [ffi.rs:21](../../crates/vpn-mobile/src/ffi.rs#L21)

- 界面与账号状态同步
  [MainActivity.kt:14](../../mobile/android/app/src/main/java/com/biubiu/vpn/MainActivity.kt#L14)

- 真实对端报文回归测试
  [packets.rs:21](../../crates/vpn-mobile/tests/packets.rs#L21)

- Android 认证竞态回归
  [ApiTest.kt:8](../../mobile/android/app/src/test/java/com/biubiu/vpn/ApiTest.kt#L8)

- 云端构建和签名产物
  [mobile.yml:1](../../.github/workflows/mobile.yml#L1)
