---
title: '在线心跳自动续期登录会话'
type: 'bugfix'
created: '2026-08-17'
status: 'done'
baseline_commit: '0896a1a84f57c15e742af17c7cb404e6e36ed0db'
context: ['{project-root}/docs/deployment.md']
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** session 登录后固定 30 天过期，网关持续心跳仍会掉线。

**Approach:** 成功刷新 Access Token 时，仅将当前 session 滑动续期 30 天。既有客户端会在心跳遇到 15 分钟 Access Token 到期时自动 refresh，无需改协议；修复后发布并部署 v0.1.8。

## Boundaries & Constraints

**Always:** 仅续期本次 Refresh Token 对应且未过期、未撤销的 session；条件更新须恰好影响 1 行；禁用用户不得续期；兼容 v0.1.6+；保留 logout、改密、管理员禁用/重置/删除及节点强制下线语义；部署保留数据库与 `vpn-data`。

**Ask First:** Token 轮换或 TTL 变化；强制下线时吊销用户全部设备；改变生产拓扑。

**Never:** heartbeat 按用户批量续期；复活过期/撤销 session；在心跳或日志暴露 Refresh Token；修改 `outputs/`。

| Scenario | State | Expected |
|---|---|---|
| 在线/重复刷新 | session 有效 | `expires_at=now+30d`，签发 Access，不新建 session |
| 过期/撤销 | 非活跃 | 不更新；TokenExpired/401 |
| 禁用 | user disabled | 不续期；AccountDisabled |
| 并发吊销 | 查询后、更新前吊销 | UPDATE 0 行；TokenExpired/401 |

</frozen-after-approval>

## Code Map

- `crates/vpn-server/src/repositories/session_repo_sqlite.rs` -- 条件续期/撤销。
- `crates/vpn-server/src/services/auth_service.rs` -- 校验、续期、签发顺序。
- `crates/vpn-server/tests/auth_flow.rs`、`crates/vpn-cli/tests/api_client.rs` -- 安全语义及旧客户端自动刷新。
- 根/桌面 Cargo、npm、Tauri 版本文件；`.github/workflows/release.yml`、`docker/Dockerfile.deploy` -- v0.1.8 发布部署。

## Tasks & Acceptance

**Execution:**
- [x] `session_repo_sqlite.rs` -- 按 token hash 原子 UPDATE，条件含 `revoked_at IS NULL AND expires_at > now`；测试有效、过期、撤销、未知、session 隔离和续期后吊销。
- [x] `auth_service.rs`、`auth_flow.rs` -- active user 校验后续期，影响 1 行才签发；覆盖禁用及吊销竞态。
- [x] `api_client.rs` -- 测试 heartbeat 401 → refresh → heartbeat 成功。
- [x] 七处版本文件与发布部署准备 -- 升级 v0.1.8，并确认现有 tag workflow、生产镜像替换及 updater manifest 路径可复用；推送和生产操作在审查通过后执行。

**Acceptance Criteria:**
- Given 网关在线且 session 有效，when 心跳跨越多次 Access 到期，then 自动续期且无需登录。
- Given session 已被 logout 或管理员动作撤销，when 再刷新，then 不恢复并按既有机制下线。
- Given v0.1.6 客户端连接新服务端，when Access 到期，then 原协议完成续期。
- Given v0.1.8 已部署，when 检查 Release、健康/版本接口及登录—心跳—吊销冒烟流程，then 产物、数据、续期和吊销均正常。

## Spec Change Log

## Design Notes

heartbeat/JWT 无 session id；按 user 续期会延长其他休眠 session。复用自动 refresh 并以 token hash 条件更新，可关闭并发吊销后的复活窗口，且无需 migration 或客户端改动。

## Verification

- `cargo fmt --all -- --check && cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`
- `cd desktop && npm ci && npm run build && cd src-tauri && cargo test && cargo clippy --all-targets -- -D warnings`
- `git diff --check`；发布后检查 workflow、`/health`、版本、日志及登录—心跳—吊销冒烟流程。

**Local result (2026-08-21):** workspace fmt/tests、`vpn-server` 严格 clippy、桌面 build/test/clippy 及 `git diff --check` 通过；全 workspace 严格 clippy 仅被基线已有的 `wg_userspace.rs:105,109` 两处 `manual_inspect` 阻断，已记入 `deferred-work.md`。

**Pending delivery gate:** v0.1.8 tag/Release、生产镜像替换、数据卷保留、updater manifest 与线上登录—心跳—吊销冒烟须在审查完成后执行和记录。

## Suggested Review Order

**续期入口与安全边界**

- 入口先准备 Access，再以原子续期结果决定是否返回。
  [`auth_service.rs:124`](../../crates/vpn-server/src/services/auth_service.rs#L124)

- 单条 SQL 同时约束 token、user、到期、吊销和账号状态。
  [`session_repo_sqlite.rs:106`](../../crates/vpn-server/src/repositories/session_repo_sqlite.rs#L106)

**端到端与兼容验证**

- 重复 refresh 证明滑动窗口且不延长其他 session。
  [`auth_flow.rs:249`](../../crates/vpn-server/tests/auth_flow.rs#L249)

- 旧客户端心跳在 401 后自动 refresh 并原请求重试。
  [`api_client.rs:265`](../../crates/vpn-cli/tests/api_client.rs#L265)

**发布版本**

- workspace 版本统一为 v0.1.8，并传播至 Cargo lockfile。
  [`Cargo.toml:13`](../../Cargo.toml#L13)

- 桌面 updater 元数据同步声明 v0.1.8。
  [`tauri.conf.json:4`](../../desktop/src-tauri/tauri.conf.json#L4)
