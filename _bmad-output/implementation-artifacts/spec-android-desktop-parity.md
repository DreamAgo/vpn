---
title: 'Android 客户端功能对齐桌面端'
type: 'feature'
created: '2026-09-16'
status: 'done'
baseline_commit: 'b83e8291e67fe562ffe36750dcbcf8f2a70fe4f1'
context: []
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** Android 虽已基于 v0.1.33，仍缺少桌面端的企业登录、更新和排障体验。

**Approach:** 补齐手机上完整的登录、连接、更新、诊断闭环，复用既有协议并适配 Android 生命周期。

## Boundaries & Constraints

**Always:** 保留密码登录、正确图标、统一版本和现有数据面；HTTPS 校验；凭据加密；日志脱敏且有界；下载前用户确认，安装交给系统。

**Ask First:** 发布正式安装包、使用生产签名或改变服务端身份/权限规则。

**Never:** 实现 iOS；绕过系统 VPN/安装授权；强制后台弹界面；将桌面开机驻留等同于手机自动连接。

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|---|---|---|---|
| 飞书登录 | 已启用/未配置 | 浏览器授权、限时轮询、保存真实账号；未配置不影响密码登录 | 取消、超时、页面重建不提交迟到会话 |
| 更新 | 配置服务的清单含新 APK | 展示版本与说明，确认后下载并显示进度 | 无 APK 明示不可用；拒绝降级、异源地址、大小/摘要/包名/签名不符 |
| 连接中操作 | 隧道运行 | 可读设置、诊断和更新检查 | 改账号、安装或退出前协调断开，避免资源竞争 |
| 排障 | 状态变化或异常 | 有界近期事件、详细日志、诊断复制 | 不记录密码、token、私钥；日志失败不阻断 VPN |

</frozen-after-approval>

## Code Map

- `desktop/src/{App.tsx,api.ts}`：用户行为基准；`desktop/src-tauri/src/commands.rs`：飞书域名校验与轮询。
- `mobile/android/app/src/main/java/com/biubiu/vpn/{Api,MainActivity,TunnelService}.kt`：现有账号、界面和隧道。
- `crates/vpn-server/src/services/client_update_service.rs`：更新镜像与 downloads 清单；现有平台更新条目仅适用于桌面。
- `.github/workflows/mobile.yml`：已支持签名构建，尚无标准发布 APK 名称约定。

## Tasks & Acceptance

**Execution:**
- [x] `Api.kt`、新增 `FeishuLogin.kt`：复用 config/start/poll 端点，固定 HTTPS 飞书域名，处理取消、超时、旋转和凭据交接；最低版本拒绝停止重试并引导升级，保留账号。
- [x] 新增 `ClientUpdates.kt`、Android manifest/资源：从配置服务读取更新，校验并下载 APK，通过受限内容 URI 调用系统安装器；处理未知来源安装授权。
- [x] `client_update_service.rs`、`.github/workflows/mobile.yml`：明确版本化 APK 命名并纳入带摘要的服务端下载清单，保留现有桌面签名校验；不自动发布。
- [x] `MainActivity.kt`、`TunnelService.kt`、新增 `Diagnostics.kt`：连接/更新/账号入口、版本与账号、IP/流量/时长、改密确认、事件和日志查看复制；提供系统 VPN 设置入口说明平台启动差异。
- [x] `mobile/android/app/src/test/`、服务端测试：覆盖上述矩阵及登录取消、恶意更新清单、日志脱敏、并发操作。
- [x] `docs/mobile.md`：功能对照、安装/签名要求及真机验证记录。

**Acceptance Criteria:**
- Given 飞书已启用，when 授权成功并回到应用，then 显示真实账号并可走原 VPN 流程。
- Given 配置服务提供高版本同签名 APK，when 用户确认更新，then 校验后进入系统安装器；拒绝安装保持当前应用可用。
- Given 正在连接，when 打开设置和诊断，then 不打断隧道且状态持续刷新。
- Given 日志含敏感错误文本，when 查看或复制，then 凭据被隐藏，容量限制生效。
- Given API 26 与 API 35，when 构建和执行主机测试，then 通过；没有真机的行为明确标为待验证。

## Spec Change Log

## Design Notes

飞书复用服务端回调和 poll token，不增加客户端 secret。更新使用现有 downloads 的摘要，另验 APK 签名和包名；Android APK 不伪装为 Tauri 更新包。桌面开机驻留/托盘由 Android 系统设置及前台通知适配，不承诺开机自动连接。

## Verification

- `cargo test --locked -p vpn-mobile -p vpn-obfs`：现有数据面回归通过。
- `cargo test --locked -p vpn-server client_update`：分发兼容与边界测试通过。
- `mobile/android/gradlew -p mobile/android assembleDebug testDebugUnitTest lintDebug`：构建测试通过，lint 无错误。
- APK 签名、版本、三 ABI 和 16 KiB 对齐检查通过。
- 真机待测：飞书回跳/旋转、安装权限/升级、锁屏、网络切换、真实内网连通。

- 本轮实现验证：25 项 Kotlin/JVM 测试、服务端 client_update 13 项测试通过（1 项真实 GitHub 下载测试未运行）；完整 debug 构建与 lint 通过。飞书、系统安装、API 26/API 35 真机行为仍待设备验证。


## Review Resolution

三路独立复核发现的改密调度、安装重试、页面重建文件冲突及连接/登录快速连点问题均按实现修补处理，未改变已批准范围。修复后复核未发现未解决问题。连接启动先保留生命周期所有权，再请求系统服务；复制详情使用快照，避免在 UI 线程等待正在执行的认证请求。下载文件按操作隔离，安装每次新建校验器，强制改密期间不自动检查更新。

最终验证：27 项 JVM 测试、17 项 vpn-mobile + 8 项 vpn-obfs 测试及移动库 Clippy 通过；服务端 client_update 13 项通过、1 项真实网络测试忽略。assembleDebug/lintDebug 通过（0 错误，8 警告）。APK v2 签名、16 KiB ZIP 对齐通过；三 ABI 齐全，打包图标与桌面源文件一致。版本 0.1.33 / versionCode 1033。未执行真实飞书、系统安装或 VPN 真机验证；未使用生产签名、发布或推送。

## Suggested Review Order

**入口与操作互斥**

- 页面和操作入口，连接期间保持可读。
  [MainActivity.kt:14](../../mobile/android/app/src/main/java/com/biubiu/vpn/MainActivity.kt#L14)

- 异步回调只接受当前操作。
  [OperationGate.kt:4](../../mobile/android/app/src/main/java/com/biubiu/vpn/OperationGate.kt#L4)

- 系统启动前同步保留连接所有权。
  [TunnelService.kt:26](../../mobile/android/app/src/main/java/com/biubiu/vpn/TunnelService.kt#L26)

**身份与更新校验**

- 服务端飞书协议及过期处理。
  [FeishuLogin.kt:7](../../mobile/android/app/src/main/java/com/biubiu/vpn/FeishuLogin.kt#L7)

- 凭据提交失败时撤销新会话。
  [Api.kt:43](../../mobile/android/app/src/main/java/com/biubiu/vpn/Api.kt#L43)

- 同源清单、文件摘要和包身份验证。
  [ClientUpdates.kt:17](../../mobile/android/app/src/main/java/com/biubiu/vpn/ClientUpdates.kt#L17)

- 每次下载独立持有文件。
  [UpdateFiles.kt:7](../../mobile/android/app/src/main/java/com/biubiu/vpn/UpdateFiles.kt#L7)

**分发与验证**

- APK 可选镜像保持桌面协议。
  [client_update_service.rs:441](../../crates/vpn-server/src/services/client_update_service.rs#L441)

- 安装身份和文件隔离回归测试。
  [UpdateSafetyTest.kt:7](../../mobile/android/app/src/test/java/com/biubiu/vpn/UpdateSafetyTest.kt#L7)

- 功能对照、签名约束和真机待测项。
  [mobile.md:52](../../docs/mobile.md#L52)
