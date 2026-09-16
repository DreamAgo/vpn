---
title: 'Android 落地已确认的蓝色手机端界面'
type: feature
created: '2026-09-16'
status: done
baseline_commit: '886f78ba13c01ad4f8c13617824f6213bde70a96'
context: []
---

<frozen-after-approval reason="用户已通过‘可以，按这个开发’批准蓝色 Demo 的实现">

## Intent

**Problem:** Android 现有四页按钮界面与已确认的手机端 Demo 不一致，连接、账号与更新信息缺少清晰层级。

**Approach:** 在现有 Kotlin 原生界面上实现蓝色 Demo 的登录、连接、活动、我的及二级弹层，以真实账号、隧道和更新状态驱动界面。

## Boundaries & Constraints

**Always:** 主色 #2563EB，浅灰背景、白卡片、正确易链图标；保留 API 26、现有凭据加密、操作互斥、系统授权、下载校验、诊断脱敏；版本读取 BuildConfig。工作树其它改动保持原样。

**Ask First:** 正式发布、生产签名或服务端身份与权限变更。

**Never:** 模拟登录、虚构在线/流量/版本/6001原因；新增 iOS；改动 VPN 数据面协议；将 Demo 场景切换控件放进正式应用。

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|---|---|---|---|
| 登录 | 无会话 | 服务地址、飞书主按钮、密码登录；登录后连接首页 | 保留取消与强制改密；真实失败文案可见 |
| 连接 | 待机、注册、握手、重连、停止 | 单一主操作与状态；仅握手后成功；时长流量来自服务 | 6001显示真实脱敏原因、提供重试与诊断 |
| 活动 | 服务日志变化 | 有界真实日志，可复制清空 | 不展示模拟记录，不泄露凭据 |
| 我的 | 已登录 | 真实账号、服务、改密、更新、诊断、系统设置、关于与退出 | 需变更会话时先协调断开 |
| 更新 | 检查、下载、校验、安装 | 二级页面/弹层；明确当前状态及可用操作 | 拒绝不合规包、取消不丢账号 |
| 生命周期 | 旋转、后台、授权回跳 | 保留现有取消过期回调机制；不闪回错误页面 | 按安全逻辑重新发起中断的操作 |

</frozen-after-approval>

## Code Map

- `mobile/android/app/src/main/java/com/biubiu/vpn/MainActivity.kt`：当前页面、操作队列、权限、会话、更新控制。
- `mobile/android/app/src/main/java/com/biubiu/vpn/TunnelService.kt`：running 包含握手前与停止中；status/details 为只读真实快照。
- `mobile/android/app/src/main/java/com/biubiu/vpn/{Api,Diagnostics,OperationGate,ClientUpdates}.kt`：保留控制面与安全边界。
- `_bmad-output/planning-artifacts/android-ux-demo/`：已批准的布局、色彩及交互参考。

## Tasks & Acceptance

**Execution:**
- [x] `MainActivity.kt` 与原生 UI 辅助文件：实现三页底栏、独立登录与弹层，接入全部现有操作，保存页面位置并保留异步控制。
- [x] `ConnectionPresentation.kt` 与对应主机测试：将运行状态与详情转为明确展示，覆盖未握手、重连、终止、过期数据。
- [x] `res/values/styles.xml`、Manifest：统一蓝色浅色主题与系统栏，兼顾键盘、系统安全区域和文字缩放。
- [x] `docs/mobile.md`：更新导航、行为与验证记录；构建测试及签名/打包校验，生成可局域网下载的调试 APK。

**Acceptance Criteria:**
- Given 已批准 Demo，when 打开正式客户端，then 品牌色、卡片、连接主按钮与三个底部导航一致，没有演示控件或数据。
- Given 未完成 WireGuard 握手或正在重连，when 刷新页面，then 不显示已连接或上一连接的流量。
- Given 登录、下载正在进行，when 快速切换页面或取消，then 不创建重复请求、不提交过期会话。
- Given 已连接，when 改密、切换服务、退出或安装，then 保留断开确认与系统权限交互。
- Given 构建环境，when 运行 JVM 测试、assembleDebug 和 lint，then 全部通过；未执行真机项目明确列出。

## Spec Change Log

## Design Notes

用户已批准视觉与交互，本规格记录实现边界，不再次索取设计确认。维持 Activity + 原生 View 架构，避免引入 UI 框架依赖。真实授权和系统安装使用 Android 原生窗口；应用二级信息使用贴底弹层。连接状态展示单独纯函数便于验证，所有副作用仍由 Activity 和 Service 持有。

## Verification

- `mobile/android/gradlew -p mobile/android assembleDebug testDebugUnitTest lintDebug`
- APK 签名与 16 KiB zipalign 校验；可用设备则运行界面检查，否则明确待真机验证。

## Review Resolution

三路复核发现并修复：更新下载期间断开入口被禁用、未登录时无法进入诊断、更新弹层进度与可用操作不明确；另处理会话失效后错误说明、系统安装返回与取消操作后的状态收尾。活动改为真实事件时间线，保留复制原始脱敏诊断。API 26 导航栏主题已独立兼容。没有改变已批准范围。

最终验证：35 项 JVM 测试全部通过；assembleDebug/lintDebug 通过（0 错误、9 警告）；APK v2 签名和 16 KiB ZIP 对齐通过，三个 ABI 齐全。调试包 0.1.33，3,351,434 字节，SHA-256 `e512bb3cd74f062b3ddbfb1f77c5f6ea4973696277fdbe7048d9e69a74b8c1a9`。本机无已连接设备或模拟器，真机视觉/系统授权/VPN 联调未执行。未发布正式包或推送。

## Suggested Review Order

- 页面与会话入口
  [MainActivity.kt:88](../../mobile/android/app/src/main/java/com/biubiu/vpn/MainActivity.kt#L88)

- 连接与更新状态
  [MainActivity.kt:225](../../mobile/android/app/src/main/java/com/biubiu/vpn/MainActivity.kt#L225)

- 握手前不展示成功或旧流量
  [ConnectionPresentation.kt:18](../../mobile/android/app/src/main/java/com/biubiu/vpn/ConnectionPresentation.kt#L18)

- 原生蓝色组件与图标
  [NativeUi.kt:11](../../mobile/android/app/src/main/java/com/biubiu/vpn/NativeUi.kt#L11)

- 连接展示边界测试
  [ConnectionPresentationTest.kt:6](../../mobile/android/app/src/test/java/com/biubiu/vpn/ConnectionPresentationTest.kt#L6)

- 平台主题兼容
  [styles.xml:1](../../mobile/android/app/src/main/res/values/styles.xml#L1)

