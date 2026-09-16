---
title: '修复飞书授权窗口重复创建失败'
type: 'bugfix'
created: '2026-09-11'
status: 'done'
baseline_commit: '15375f0fde8ab36c15e6b5e99c89b21a10e24480'
context:
  - '{project-root}/_bmad-output/implementation-artifacts/spec-client-feishu-login.md'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** 桌面客户端重新发起飞书登录时，旧的 `feishu-auth` WebView 虽已收到关闭请求，但可能尚未从 Tauri 窗口注册表移除；客户端立即用相同 label 创建窗口会报 `a webview with label 'feishu-auth' already exists`，用户无法继续授权。

**Approach:** 创建授权窗口前先关闭已有同名窗口，并在有限时间内等待其确实销毁后再创建；登录结束时统一请求关闭。销毁超时或关闭失败时返回可操作的中文错误，不继续创建冲突窗口。

## Boundaries & Constraints

**Always:** 保留一个飞书登录流程和一个授权窗口的互斥约束；确认旧窗口已从 Tauri 管理器移除后才复用固定 label；等待必须有较短上限且不阻塞 UI 线程；成功、错误和超时退出时都请求清理授权窗口；错误信息不得包含 OAuth token 或敏感 URL 参数。

**Ask First:** 改用随机窗口 label、允许多个并发飞书授权、改变 OAuth 轮询协议或服务端接口。

**Never:** 用无上限轮询等待窗口消失；忽略关闭失败后强行创建同名窗口；改动服务端飞书配置或当前主工作区内未提交的飞书设置功能。

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| 无旧窗口 | 未注册 `feishu-auth` | 立即创建授权窗口 | N/A |
| 旧窗口正常关闭 | 注册表中已有同名窗口 | 请求关闭，确认移除后创建新窗口 | N/A |
| 旧窗口迟迟未销毁 | 超过限定等待时间仍存在 | 不创建新窗口 | 返回“旧授权窗口未能关闭，请手动关闭后重试”类中文错误 |
| 授权流程结束 | 成功、轮询错误或超时 | guard 请求关闭当前授权窗口 | 保留原业务错误，清理失败不覆盖根因 |

</frozen-after-approval>

## Code Map

- `desktop/src-tauri/src/commands.rs` -- 飞书登录互斥、授权窗口创建、轮询和 RAII 清理均集中在此；当前关闭后立即创建导致竞态。
- `desktop/src-tauri/src/lib.rs` -- 仅主窗口拦截关闭事件；授权窗口允许真正销毁并释放固定 label。
- `_bmad-output/implementation-artifacts/spec-client-feishu-login.md` -- 既有飞书客户端登录安全边界和验证背景。

## Tasks & Acceptance

**Execution:**
- [x] `desktop/src-tauri/src/commands.rs` -- 提取可测试的有限重试策略，并实现“关闭后等待窗口 label 释放”的异步辅助函数；在构建授权窗口前调用它。
- [x] `desktop/src-tauri/src/commands.rs` -- 增加重试边界单元测试，覆盖立即释放、延迟释放和超时，防止再次退化为先关后立即建。

**Acceptance Criteria:**
- Given 客户端注册表中遗留同名授权窗口，when 用户重新发起飞书登录，then 客户端等待旧窗口销毁并成功打开新的授权窗口，不再出现 label 已存在错误。
- Given 旧窗口在上限内无法销毁，when 用户重新发起登录，then 客户端停止创建并显示明确的中文重试提示，主窗口和既有登录状态不受影响。
- Given 没有遗留授权窗口，when 用户发起飞书登录，then 原有窗口尺寸、父窗口、HTTPS 导航限制和 OAuth 轮询行为保持不变。

### Review Findings

- [x] [Review][Patch][P1] 全局 `CloseRequested` 处理器阻止 `feishu-auth` 真正销毁，核心修复无效 [`desktop/src-tauri/src/lib.rs:297`]
- [x] [Review][Patch][P2] 测试仅覆盖通用轮询器，未覆盖窗口 label 的关闭策略 [`desktop/src-tauri/src/commands.rs:394`]
- [x] [Review][Patch][P3] 规格中的完整 `baseline_commit` 无法解析，影响后续差异审查 [`_bmad-output/implementation-artifacts/spec-fix-feishu-auth-window-recreate.md:6`]
- [x] [Review 2][Patch][P1] 用户关闭授权窗口后轮询仍在后台继续并阻止立即重试 [`desktop/src-tauri/src/commands.rs:202`]

## Spec Change Log

- 2026-09-11：三层审查后将“关闭只隐藏”限定为主窗口，补充临时窗口关闭策略测试，并修正基线提交。
- 2026-09-11：第二轮审查补充授权窗口关闭检测，用户取消后及时释放单飞锁；若授权同时完成则撤销远端会话。

## Design Notes

Tauri `WebviewWindow::close()` 只提交关闭请求；真正移除窗口发生在后续 `Destroyed` 事件处理期间。使用短间隔查询 `AppHandle::get_webview_window(label)` 可以直接验证影响 label 复用的真实条件，也避免单纯固定延时在慢机器上再次竞态。重试策略拆成不依赖 Tauri 的小函数，以确定性单元测试验证次数和超时边界。

## Verification

**Commands:**
- `cargo fmt --all -- --check` -- expected: Rust 格式检查通过。
- `cargo test --manifest-path desktop/src-tauri/Cargo.toml commands::tests` -- expected: URL、互斥和窗口等待策略测试全部通过。
- `cargo clippy --manifest-path desktop/src-tauri/Cargo.toml --all-targets -- -D warnings` -- expected: 本次桌面端代码无新增警告。

**Manual checks:**
- 连续完成、取消并再次发起飞书授权，均只能出现一个授权窗口，且不再提示 `feishu-auth already exists`。

**Review verification (2026-09-11):** 桌面端命令测试 7 项与库测试 24 项全部通过；Clippy `-D warnings`、格式及 diff 检查通过。

## Suggested Review Order

**窗口重建生命周期**

- 入口先释放旧窗口，再创建远端授权会话和新窗口。
  [`commands.rs:161`](../../desktop/src-tauri/src/commands.rs#L161)

- 有限轮询确认固定 label 已释放，失败时返回明确错误。
  [`commands.rs:76`](../../desktop/src-tauri/src/commands.rs#L76)

- 通用等待器提供非阻塞、有限次数的状态确认。
  [`commands.rs:61`](../../desktop/src-tauri/src/commands.rs#L61)

**清理与回归保护**

- 仅主窗口关闭时隐藏，临时授权窗口正常销毁。
  [`lib.rs:299`](../../desktop/src-tauri/src/lib.rs#L299)

- 轮询检测用户关闭窗口，完成竞态时撤销远端会话。
  [`commands.rs:202`](../../desktop/src-tauri/src/commands.rs#L202)

- 成功路径显式交给 guard 单次关闭，再显示主窗口。
  [`commands.rs:249`](../../desktop/src-tauri/src/commands.rs#L249)

- 三类测试锁定立即、延迟与超时边界行为。
  [`commands.rs:394`](../../desktop/src-tauri/src/commands.rs#L394)
