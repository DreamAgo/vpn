---
title: '飞书审批外部选项：用户组列表'
type: 'feature'
created: '2026-07-31'
status: 'done'
baseline_commit: 'f4c409268d4dfc2c20132bbf2ae7b2be9bca8bc0'
context:
  - '{project-root}/_bmad-output/implementation-artifacts/spec-feishu-approval-subnet-options.md'
  - '{project-root}/docs/external-api.md'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** 飞书审批外部选项入口目前只注册 `subnets`，审批表单无法动态选择易链中已经存在的用户组。

**Approach:** 在现有 provider 注册模型中增加 `user-groups` 数据源，从用户组服务读取全部用户组，并复用既有 token 鉴权、搜索、签名游标分页、国际化和飞书响应协议。

## Boundaries & Constraints

**Always:** 使用 `POST /api/v1/integrations/feishu/approval-options/user-groups`；选项 `id` 使用数据库中稳定的用户组 ID；显示文案使用用户组名称；返回全部用户组并按现有统一逻辑排序、搜索和分页；与 `subnets` 共用 `VPN_FEISHU_APPROVAL_OPTIONS_TOKEN`。

**Ask First:** 若需要按审批发起人过滤用户组、在文案中暴露成员或路由信息、改变用户组表结构或新增独立 token，必须先征得用户同意。

**Never:** 不复制鉴权、分页或 HTTP 响应逻辑；不要求 JWT 管理端认证；不改变现有用户组 CRUD、`subnets` 数据源或飞书明文返回约定。

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|---------------|----------------------------|----------------|
| 获取用户组 | token 正确且存在用户组 | 返回稳定 ID 和用户组名称，例如“测试” | N/A |
| 搜索用户组 | `query` 匹配名称 | 只返回名称包含查询词的组 | 无匹配时成功返回空数组 |
| 无用户组 | `user_groups` 表为空 | 返回空数组且 `hasMore=false` | N/A |
| 鉴权失败 | token 缺失或错误 | 不访问 provider | HTTP 401，飞书格式非零 `code` |
| 跨源游标 | 使用 `subnets` 游标请求 `user-groups` | 拒绝游标，不能跨数据源复用 | HTTP 400 |

</frozen-after-approval>

## Code Map

- `crates/vpn-server/src/services/external_options_service.rs` -- 增加用户组 provider，将用户组 DTO 映射为中立选项。
- `crates/vpn-server/src/main.rs` -- 将现有 `UserGroupService` 注册为 `user-groups` 数据源。
- `crates/vpn-server/tests/external_options_flow.rs` -- 构造用户组并验证响应、搜索及跨源游标隔离。
- `crates/vpn-server/src/handlers/openapi.rs`、`docs/external-api.md` -- 发布新增 source 和调用示例。

## Tasks & Acceptance

**Execution:**
- [x] `crates/vpn-server/src/services/external_options_service.rs` -- 实现 `UserGroupExternalOptionProvider`，通过现有服务列出组并映射稳定 ID/名称。
- [x] `crates/vpn-server/src/services/mod.rs`、`crates/vpn-server/src/main.rs` -- 导出并注册 `user-groups` provider，复用已经初始化的用户组服务。
- [x] `crates/vpn-server/tests/external_options_flow.rs` -- 扩展测试装配，覆盖用户组成功、搜索、空列表和跨源游标。
- [x] `crates/vpn-server/src/handlers/openapi.rs`、`docs/external-api.md` -- 将支持的数据源和用户组 URL 加入契约文档。

**Acceptance Criteria:**
- Given 线上已有“测试”和“测试 2”两个用户组，when 用正确 token 请求 `user-groups`，then 返回两个以组 ID 为 `id`、组名为显示文案的选项。
- Given 已上线的 `subnets` 地址，when 本功能部署，then其请求结构、认证和响应保持兼容。
- Given 后续新增第三种目录数据源，when 实现 provider 并注册，then仍无需增加新的公开路由或复制协议处理。

## Spec Change Log

## Design Notes

使用路径中的 kebab-case `user-groups` 作为稳定外部数据源标识。Provider 只负责把用户组转换成 `{id,label,is_default}`；外层服务继续负责排序、`query` 搜索、50 项分页、HMAC 游标和 `zh_cn` i18n，保持每个数据源实现最小化。

## Verification

**Commands:**
- `cargo fmt --all -- --check` -- Rust 格式正确。
- `cargo test -p vpn-server external_options` -- 两个 provider 及协议错误分支通过。
- `cargo test -p vpn-server` -- 服务端测试无回归。
- `cargo clippy -p vpn-server --all-targets -- -D warnings` -- 无新增 lint。

## Suggested Review Order

**数据源扩展与装配**

- 统一 provider 边界将用户组转换为稳定外部选项。
  [`external_options_service.rs:65`](../../crates/vpn-server/src/services/external_options_service.rs#L65)

- 启动时注册 `user-groups`，复用现有用户组服务和公共路由。
  [`main.rs:79`](../../crates/vpn-server/src/main.rs#L79)

**公开契约**

- OpenAPI 枚举同步发布两个受支持的数据源。
  [`openapi.rs:99`](../../crates/vpn-server/src/handlers/openapi.rs#L99)

- 运维文档给出用户组地址和共享 Token 配置。
  [`external-api.md:91`](../../docs/external-api.md#L91)

**验证与后续风险**

- 端到端覆盖稳定 ID、搜索、空集和跨源游标。
  [`external_options_flow.rs:135`](../../crates/vpn-server/tests/external_options_flow.rs#L135)

- 既有 offset 游标一致性风险单独延期，不扩大本次范围。
  [`deferred-work.md:24`](deferred-work.md#L24)
