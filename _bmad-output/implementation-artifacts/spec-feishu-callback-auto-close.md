---
title: '飞书授权成功页自动关闭'
type: 'feature'
created: '2026-07-31'
status: 'done'
baseline_commit: '9d5779cab290e1a1e1a60ffbbfa4bc328306ab19'
context:
  - '{project-root}/_bmad-output/implementation-artifacts/spec-client-feishu-login.md'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** 飞书授权完成后，浏览器仍停留在“可以关闭此窗口并返回客户端”页面，用户需要手动关闭，打断从浏览器返回桌面客户端的流程。

**Approach:** 成功回调页加载后以客户端脚本尝试自动关闭当前窗口；如果浏览器安全策略不允许脚本关闭，则继续显示明确的手动关闭提示。失败回调页保持可见，不自动关闭，便于用户看到错误并重试。

## Boundaries & Constraints

**Always:** 只在服务端已成功处理飞书回调后尝试关闭；回调页继续保持无本站 token、飞书 token 或用户资料；页面无需加载外部脚本；自动关闭失败时必须有可读兜底提示；保留 UTF-8 和明确页面标题。

**Ask First:** 引入桌面自定义 URL Scheme、浏览器扩展或其他深链机制；改变 OAuth 轮询、会话签发或自动创建账号流程；让失败页面也自动关闭。

**Never:** 宣称所有浏览器都能保证自动关闭；绕过浏览器安全策略；把凭证放入 HTML、查询参数或客户端脚本；因关闭失败而将成功回调改判为失败。

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| 授权成功且浏览器允许关闭 | 飞书回调校验成功 | 页面提示成功，并在短延迟后调用 `window.close()` | 无需额外动作 |
| 授权成功但浏览器禁止关闭 | 非脚本打开的标签页或浏览器策略拦截 | 页面保持打开，显示“如未自动关闭，请手动关闭并返回客户端” | 不影响客户端轮询领取登录结果 |
| 授权失败 | state、code 或飞书错误无效 | 显示现有失败提示，不执行自动关闭脚本 | 用户关闭页面并在客户端重试 |

</frozen-after-approval>

## Code Map

- `crates/vpn-server/src/handlers/auth.rs` -- 生成飞书 OAuth 成功/失败回调 HTML；本次在成功分支加入无外部依赖的关闭脚本与兜底文案。
- `_bmad-output/implementation-artifacts/spec-client-feishu-login.md` -- 已完成的飞书登录安全边界与回调协议背景。

## Tasks & Acceptance

**Execution:**
- [x] `crates/vpn-server/src/handlers/auth.rs` -- 将成功与失败 HTML 提取为可验证的静态页面内容，在成功页短延迟调用 `window.close()`，失败页不调用。
- [x] `crates/vpn-server/src/handlers/auth.rs` 测试模块 -- 断言成功页包含自动关闭脚本和兜底提示，失败页不包含自动关闭调用，防止后续回归。

**Acceptance Criteria:**
- Given 飞书 OAuth 回调处理成功，when 浏览器渲染响应页，then 页面自动尝试关闭且客户端原有轮询流程不受影响。
- Given 浏览器阻止脚本关闭，when 自动关闭未发生，then 用户仍能看到可手动关闭并返回客户端的提示。
- Given 飞书 OAuth 回调处理失败，when 浏览器渲染响应页，then 页面保留失败原因方向的提示且不会自动消失。

## Spec Change Log

## Design Notes

浏览器只允许脚本可靠关闭由脚本打开的窗口；通过系统默认浏览器打开的 OAuth 标签页可能被 Chrome、Safari 等拒绝关闭。因此实现采用“尽力关闭 + 永久可见兜底提示”，不改变服务端成功状态，也不引入自定义协议。短延迟让用户能够看到授权成功反馈，并给客户端轮询留出正常处理时间。

## Verification

**Commands:**
- `cargo fmt --all -- --check` -- expected: Rust 格式检查通过。
- `cargo test -p vpn-server handlers::auth` -- expected: 成功/失败回调页面契约测试通过。
- `cargo test -p vpn-server` -- expected: 服务端现有测试无回归。

**Manual checks:**
- 在默认浏览器完成一次飞书登录：允许脚本关闭时页面自动关闭；被浏览器拦截时仍能手动关闭，桌面客户端均应完成登录。

## Suggested Review Order

**回调行为**

- 成功页先展示兜底提示，再延迟尝试关闭浏览器窗口。
  [`auth.rs:81`](../../crates/vpn-server/src/handlers/auth.rs#L81)

- 单一映射函数确保只有成功回调选择自动关闭页面。
  [`auth.rs:109`](../../crates/vpn-server/src/handlers/auth.rs#L109)

- Handler 保持 OAuth 处理与页面选择边界清晰。
  [`auth.rs:119`](../../crates/vpn-server/src/handlers/auth.rs#L119)

**契约与后续**

- 契约测试锁定延迟、兜底文案和失败页不关闭。
  [`auth.rs:290`](../../crates/vpn-server/src/handlers/auth.rs#L290)

- 既有无效 query 的统一失败页改进单独延期处理。
  [`deferred-work.md:20`](deferred-work.md#L20)
