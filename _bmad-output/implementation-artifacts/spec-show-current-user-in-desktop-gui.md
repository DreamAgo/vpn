---
title: '桌面 GUI 显示当前登录用户'
type: 'feature'
created: '2026-07-17'
status: 'done'
baseline_commit: '7054d061fcc44f9932edb1c42be3c051fb9f4a25'
context:
  - '{project-root}/_bmad-output/implementation-artifacts/spec-desktop-client-operability.md'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** 桌面客户端登录后只显示链路与服务端信息，用户无法确认当前使用的是哪个账号，切换账号或排障时容易混淆。

**Approach:** 复用成功登录时已经持久化的用户名，通过只读 Tauri 命令提供给 React；在主状态卡与“账号”设置页显示当前用户名，并在登出或本地会话失效时立即清空，避免展示旧账号。

## Boundaries & Constraints

**Always:** 用户名以现有凭证仓库为唯一来源；只读取用户名，不向前端暴露 refresh token、密码或其他凭证；登录成功后无需重启即可显示；登出、强制下线与修改密码导致的登出必须清空 UI 中的旧用户名；布局在窄窗口中不得溢出，长用户名应截断并保留可读提示。

**Ask First:** 若实现需要修改认证存储格式、服务端认证 API、登录响应结构或引入新的用户资料接口，必须先征得用户确认。

**Never:** 不解析 JWT 获取用户名；不把用户名写入新的独立缓存；不改变登录、刷新、连接或登出的既有语义；不在日志或诊断复制内容中新增凭证信息。

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| 正常登录 | 凭证仓库包含 refresh token 与用户名 | 主界面和账号设置显示该用户名 | N/A |
| 旧版凭证 | 已登录但本地没有用户名键 | 界面显示“未知账号”，连接功能仍可使用 | 不因读取失败退出或阻断连接 |
| 会话结束 | 用户登出、强制下线或改密后登出 | 用户名随登录态一起清空，返回登录页 | 不短暂残留上一个账号 |
| 长用户名 | 用户名超过可用宽度 | 单行省略显示，完整值通过 title 可查看 | 不撑破状态卡或设置面板 |

</frozen-after-approval>

## Code Map

- `crates/vpn-cli/src/config.rs` -- 已有 `CredentialRepo::username()` 与登录保存、登出清除逻辑，是当前账号的唯一可信来源。
- `desktop/src-tauri/src/commands.rs` -- 新增只读用户名命令，统一处理仓库不可用或用户名缺失。
- `desktop/src-tauri/src/lib.rs` -- 将用户名命令注册到 Tauri invoke handler。
- `desktop/src/api.ts` -- 声明前端用户名查询封装，并提供浏览器预览值。
- `desktop/src/App.tsx` -- 随登录态刷新用户名，在主状态卡和账号页展示，并在所有登出路径清空。
- `desktop/src/styles.css` -- 为账号行和长用户名增加紧凑、可截断的响应式样式。

## Tasks & Acceptance

**Execution:**
- [x] `desktop/src-tauri/src/commands.rs`、`desktop/src-tauri/src/lib.rs` -- 暴露并注册 `saved_username` 只读命令，返回 `Option<String>`，不改变凭证结构。
- [x] `desktop/src/api.ts` -- 增加 `savedUsername()`，Tauri 调用后端，预览模式返回示例账号。
- [x] `desktop/src/App.tsx` -- 增加当前用户状态，与登录态、服务端一起刷新；在无会话与所有登出路径清空；主状态卡及账号设置页显示用户名。
- [x] `desktop/src/styles.css` -- 实现账号文本截断和窄窗口布局，保持现有视觉体系。
- [x] 前后端验证 -- 覆盖用户名存在、缺失、登出清空和长用户名布局相关路径。

**Acceptance Criteria:**
- Given 用户以 `szjx` 登录成功，when 主界面完成首次刷新，then 链路状态附近明确显示“当前账号 szjx”，账号设置页也显示 `szjx`。
- Given 用户主动登出、被强制下线或修改密码后重新登录被要求，when 界面返回登录页，then 已登录区域不再保留此前用户名。
- Given 本地为旧版凭证且没有用户名，when 客户端判断会话仍有效，then 界面显示“未知账号”且连接、断开和设置操作保持可用。
- Given 用户名很长，when 窗口处于最窄支持宽度，then 文本省略且界面无横向溢出，悬停可查看完整用户名。

## Spec Change Log

## Design Notes

用户名查询应与 `isLoggedIn()`、`savedServer()` 在同一次刷新中并行完成。UI 状态使用 `string | null`：`null` 表示未登录或尚未获得账号；已登录但用户名缺失时渲染“未知账号”。这样可区分“没有会话”和“兼容旧凭证”，同时不创建第二份持久化来源。

## Verification

**Commands:**
- `cargo fmt --check --manifest-path desktop/src-tauri/Cargo.toml` -- expected: Rust 格式检查通过。
- `cargo test --manifest-path desktop/src-tauri/Cargo.toml` -- expected: 桌面后端与依赖测试全部通过。
- `cargo clippy --manifest-path desktop/src-tauri/Cargo.toml --all-targets -- -D warnings` -- expected: 无 Clippy 警告。
- `npm run build --prefix desktop` -- expected: TypeScript 类型检查与 Vite 生产构建通过。

**Results (2026-07-17):**
- Rust 格式检查通过；桌面后端测试 `3 passed`；严格 Clippy 零警告。
- 前端 TypeScript 与 Vite 生产构建通过；`vpn-cli` 凭证仓库测试 `4 passed`；`git diff --check` 通过。
- 独立核对全部 `setLoggedIn(false)` 路径均同步清空用户名；长用户名 DOM 保留 `title`，CSS 使用 `min-width: 0`、`text-overflow: ellipsis` 与全局 `border-box`。项目暂未配置前端组件测试框架，因此没有截图级自动化回归。

**Manual checks (if no CLI):**
- 在浏览器预览和窄窗口下确认示例账号可见、长文本省略、账号设置页一致，登出后返回登录页且不显示旧账号。

## Suggested Review Order

**会话一致性**

- 从统一刷新入口理解账号与登录态如何避免旧请求覆盖。
  [`App.tsx:93`](../../desktop/src/App.tsx#L93)

- 会话结束先使在途轮询失效，再清理本地凭证。
  [`App.tsx:262`](../../desktop/src/App.tsx#L262)

- 登录成功立即进入主界面，由非阻塞刷新补齐账号。
  [`App.tsx:341`](../../desktop/src/App.tsx#L341)

**凭证桥接**

- 仅活动本地会话可读取既有用户名键。
  [`commands.rs:104`](../../desktop/src-tauri/src/commands.rs#L104)

- Tauri 注册只读用户名命令。
  [`lib.rs:175`](../../desktop/src-tauri/src/lib.rs#L175)

- 前端调用独立降级，不阻断连接状态刷新。
  [`api.ts:132`](../../desktop/src/api.ts#L132)

**界面呈现**

- 主状态卡直接展示当前账号。
  [`App.tsx:392`](../../desktop/src/App.tsx#L392)

- 复用组件统一未知账号、截断与完整提示。
  [`App.tsx:570`](../../desktop/src/App.tsx#L570)

- 账号设置页复用同一当前用户状态。
  [`App.tsx:759`](../../desktop/src/App.tsx#L759)

- 弹性布局保证长用户名不撑破窄窗口。
  [`styles.css:329`](../../desktop/src/styles.css#L329)

**后续运维**

- 记录既有登出网络请求缺少超时的问题。
  [`deferred-work.md:7`](deferred-work.md#L7)
