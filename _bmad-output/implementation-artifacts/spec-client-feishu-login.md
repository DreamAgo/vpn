---
title: '桌面客户端接入飞书登录'
type: 'feature'
created: '2026-07-30'
status: 'done'
baseline_commit: '3897dde3a1e2ae1d6c1d87d36135793a57cf94c8'
context:
  - '{project-root}/docs/architecture.md'
  - '{project-root}/docs/client.md'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** 桌面客户端目前只能用本地用户名密码登录，企业成员无法复用飞书身份。

**Approach:** 保留密码登录并增加飞书 OAuth。桌面端用系统浏览器授权；服务端交换授权码，优先使用已有身份绑定或邮箱账号，否则自动创建受限普通账号，再签发现有格式的 access/refresh token。

## Boundaries & Constraints

**Always:** App Secret 仅存服务端；校验高熵、短时、一次性的 state 与独立轮询凭证；首次优先按飞书邮箱绑定唯一现有用户，无匹配时在同一事务创建 `user` 角色、1 台设备、无用户组的账号并绑定 `(provider, subject)`；生成不可知的随机密码哈希且无需改密；用户名取邮箱前缀并在冲突时追加 subject 短哈希；回调页不含 token；敏感字段不入日志；密码登录和凭证格式保持兼容。

**Ask First:** 改变自动账号的角色、设备数或默认用户组；按手机号/姓名匹配；禁用密码登录；引入外部身份服务。

**Never:** 通过飞书自动创建管理员；向桌面端提供飞书密钥/token；信任客户端用户资料；绕过禁用状态或设备配额。

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|---|---|---|---|
| 配置 | 完整/缺失 | 启用/禁用飞书入口 | 密码登录不受影响 |
| 首次授权 | 邮箱匹配/无匹配 | 绑定已有账号或自动创建普通账号，再签发会话 | 无邮箱、身份冲突或已有账号禁用时拒绝 |
| 再次授权 | subject 已绑定 | 直接登录绑定用户 | 邮箱变化不改绑 |
| 回调领取 | state、poll token 有效 | 回调无 token；客户端领取一次 | 错误、过期、取消或重放均拒绝 |

</frozen-after-approval>

## Code Map

- `crates/vpn-api-types/src/auth.rs`、`crates/vpn-server/src/{config.rs,state.rs,app.rs,main.rs}` -- 协议、配置、路由和装配。
- `crates/vpn-server/src/{handlers/auth.rs,services/auth_service.rs,services/feishu_auth_service.rs}` -- OAuth、绑定、会话与审计。
- `crates/vpn-server/src/repositories/user_repo_sqlite.rs`、`migrations/*_external_identities.sql` -- 事务化账号创建、邮箱查找与身份绑定。
- `crates/vpn-cli/src/api.rs`、`desktop/src-tauri/src/{commands.rs,lib.rs}`、`desktop/src/{api.ts,App.tsx,styles.css}` -- 浏览器、轮询、凭证和 UI。

## Tasks & Acceptance

**Execution:**
- [x] `vpn-api-types`、服务端配置/路由/装配 -- 定义探测、发起、回调、轮询协议及配置校验。
- [x] 迁移、用户仓储 -- 增加唯一身份绑定、邮箱查找及事务化自动创建，测试并发、用户名和身份冲突。
- [x] 飞书/Auth 服务与 handler -- 实现可模拟 HTTP 的一次性状态机、绑定/自动创建、会话、审计及矩阵测试。
- [x] CLI API、Tauri bridge -- 在真实用户 GUI 打开浏览器，限时轮询并按原格式保存凭证。
- [x] 桌面 UI、`docs/{client.md,external-api.md}` -- 增加状态反馈，记录邮箱权限、回调白名单和环境变量。

**Acceptance Criteria:**
- Given 服务端配置正确且邮箱无匹配，when 用户首次完成飞书授权，then 自动创建受限普通账号、绑定飞书身份并进入登录态。
- Given 邮箱已匹配现有启用用户，when 首次授权，then 绑定该账号而不重复创建用户。
- Given 飞书未配置，when 打开登录页，then 密码登录可用且飞书入口明确不可用。
- Given state、授权码或领取结果被重放，when 再次调用，then 服务端拒绝且不签发第二个会话。
- Given 已绑定用户被禁用，when 再次授权，then 登录被拒绝且客户端留在登录页。

## Spec Change Log

## Design Notes

采用固定 HTTPS 服务端回调与桌面短轮询。回调仅记录短 TTL 的已验证身份；原客户端凭 poll token 原子消费时，才在事务中绑定/创建用户并签发会话。新账号无默认用户组，因此管理员授权网段前只能完成登录和设备接入，不能获得额外业务网段访问权。

## Verification

**Commands:**
- `cargo fmt --all -- --check && cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`
- `cd desktop && npm run build`
- `cd desktop/src-tauri && cargo test && cargo clippy --all-targets -- -D warnings`

## Suggested Review Order

**OAuth 状态机与安全边界**

- 从核心状态机理解限流、回调、一次性领取与失败恢复。
  [`feishu_auth_service.rs:185`](../../crates/vpn-server/src/services/feishu_auth_service.rs#L185)

- 公开端点仅编排服务并控制审计写入。
  [`auth.rs:51`](../../crates/vpn-server/src/handlers/auth.rs#L51)

- 路由集中展示完整浏览器授权协议。
  [`app.rs:53`](../../crates/vpn-server/src/app.rs#L53)

**身份绑定与自动建号**

- 事务化解析身份、邮箱唯一性与冲突安全用户名。
  [`user_repo_sqlite.rs:205`](../../crates/vpn-server/src/repositories/user_repo_sqlite.rs#L205)

- 唯一 provider/subject 约束固定外部身份归属。
  [`external_identities.sql:1`](../../migrations/20260730120000_external_identities.sql#L1)

- 复用既有 JWT 与 refresh-session 生命周期。
  [`auth_service.rs:28`](../../crates/vpn-server/src/services/auth_service.rs#L28)

**桌面授权与凭证交接**

- Tauri 固定 HTTPS/飞书域名并互斥执行授权轮询。
  [`commands.rs:75`](../../desktop/src-tauri/src/commands.rs#L75)

- UI 保留密码登录并呈现飞书等待与错误状态。
  [`App.tsx:1098`](../../desktop/src/App.tsx#L1098)

- CLI 复用统一信封解析并同步领取后的 token。
  [`api.rs:138`](../../crates/vpn-cli/src/api.rs#L138)

**契约、配置与运维说明**

- DTO 明确 pending/complete 与真实用户名返回。
  [`auth.rs:62`](../../crates/vpn-api-types/src/auth.rs#L62)

- 服务端配置采用全有或全无的启用语义。
  [`config.rs:47`](../../crates/vpn-server/src/config.rs#L47)

- 部署文档说明权限、回调与自动账号权限边界。
  [`client.md:39`](../../docs/client.md#L39)

**Result (2026-07-30):** fmt、workspace tests、桌面 build/test/clippy 与本次改动范围 clippy 均通过。workspace clippy 被基线文件 `crates/vpn-cli/src/wg_userspace.rs` 的两处 `clippy::manual_inspect` 阻断；使用 `-A clippy::manual_inspect` 验证 `vpn-cli` 其余目标通过。
