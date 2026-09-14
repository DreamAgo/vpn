---
title: '飞书审批手动订阅与执行记录'
type: 'feature'
created: '2026-09-14'
status: 'done'
baseline_commit: 'c4ce64622fb808555203d82116b21ed4c8a7370d'
context: []
---

<frozen-after-approval reason="用户已授权增加手动订阅按钮并判断是否执行过">

## Intent

**Problem:** 管理员需要手动取得 tenant token 并调用审批定义订阅接口，后台无法判断是否成功执行过。

**Approach:** 在集成设置的飞书审批卡片新增订阅按钮，通过管理员接口调用飞书订阅，并持久化对应应用和审批定义的最近成功时间。明确显示本系统记录，不能把无记录解释为飞书未订阅，也不能把历史成功解释为实时订阅状态。

## Boundaries & Constraints

**Always:** 使用已应用的配置；保存后尚未重启时禁止执行；管理员鉴权；秘密留在服务端；持久化成功结果并按 App ID 与审批 Code 隔离。失败不覆盖历史成功，可再次手动执行。前端有未保存输入或请求进行中时禁止重复操作。读取状态没有外部副作用。

**Ask First:** 部署线上或改变既有网络授权与撤权语义。

**Never:** 自动订阅、自动重启、修改已有用户网络权限、将历史记录伪装成飞书实时状态、把上游响应正文或 token 写入错误响应。

## I/O & Edge-Case Matrix

| 场景 | 输入/状态 | 预期行为 | 错误处理 |
|---|---|---|---|
| 首次成功 | 登录审批已启用、配置已应用 | 调用飞书并记录成功时间 | 飞书失败时返回脱敏错误 |
| 再次执行 | 已有成功记录 | 仍可调用飞书、确认后更新时间 | 不根据历史记录跳过实际调用 |
| 未配置 | 审批或登录关闭 | 禁止操作 | 服务端明确拒绝 |
| 待重启 | desired 不同于 applied | 不请求飞书 | 提示先重启 |
| 配置切换 | App ID 或 Code 改变并重启 | 只展示新组合的历史记录 | 不沿用其他组合的成功状态 |
| 服务重启 | 同一应用与定义 | 保留成功时间 | 无记录显示本系统暂无成功记录 |
| 非管理员 | 未登录或普通用户 | 禁止 GET/POST | 401/403 |

</frozen-after-approval>

## Code Map

- `crates/vpn-server/src/services/integration_settings_service.rs`：applied/desired 边界、system_config 存储、订阅编排。
- `crates/vpn-server/src/services/feishu_approval_service.rs`：复用飞书凭据与 tenant token 请求，新增订阅调用。
- `crates/vpn-server/src/handlers/system.rs`、`crates/vpn-server/src/app.rs`：管理员 GET/POST 路由。
- `crates/vpn-api-types/src/system.rs`：安全状态 DTO。
- `frontend/src/pages/IntegrationSettingsPage.tsx`、`frontend/src/services/auth.ts`、`frontend/src/types/api.ts`：按钮、执行记录、错误提示和 API 契约。
- `crates/vpn-server/tests/integration_settings_flow.rs`：路由鉴权与配置边界验证。
- `docs/external-api.md`：手动订阅运维步骤及状态局限。

## Tasks & Acceptance

**Execution:**
- [x] 服务与 DTO -- 增加订阅操作、持久化和状态查询。
- [x] 管理接口与前端 -- 增加受权限保护的按钮并展示成功时间。
- [x] 单元及流程测试 -- 验证成功、失败、配置切换、重启与鉴权。
- [x] 文档 -- 更新部署步骤和记录语义。

**Acceptance Criteria:**
- Given 已启用并应用审批配置，when 管理员点击订阅，then 飞书确认成功后页面显示当前应用、定义及成功时间。
- Given 本系统没有历史记录，when 打开页面，then 页面不宣称飞书当前未订阅。
- Given 本次订阅失败且曾经成功，when 刷新页面，then 历史成功记录仍可见且本次错误有提示。

## Spec Change Log

## Design Notes

订阅记录与配置分开保存，避免一次网络操作制造待重启状态。状态指向当前 applied 配置；前后端同时拒绝待重启操作，避免订阅错误定义。当前工作区已有其他改动，逐块追加并保留现有代码。

## Verification

- `cargo test -p vpn-server integration_settings`：服务与路由测试通过。
- 前端构建与 TypeScript 检查通过。
- 定向审查错误脱敏、状态语义、配置切换和请求期间输入保护。

## Review Notes

- 独立盲审与验收审查未发现未解决的后端问题。请求显式发送 JSON，保留上游数字错误码但不暴露响应正文。
- 边界审查发现重启且应用/审批 Code 不变时状态缓存不刷新；查询 key 已包含启用状态和待重启状态，修复按钮错误禁用。
- 工作区已有其他未提交修改，本次逐块追加，保留用户现有改动；未部署线上。

## Verification Results

- 集成设置服务单元测试 8 项通过；订阅分类与持久化定向测试 2 项通过（持久化用例与前项重叠）。
- 完整 `integration_settings_flow` 流程测试 1 项通过，覆盖管理员、普通用户、匿名访问及配置未应用拒绝操作。
- `npm run build` 通过；定向 `git diff --check` 通过。
- 未调用线上订阅接口，未部署。

## Suggested Review Order

- 订阅入口、执行记录与重启后查询状态。
  [IntegrationSettingsPage.tsx:73](../../frontend/src/pages/IntegrationSettingsPage.tsx#L73)

- 已应用配置约束、并发保护和成功记录持久化。
  [integration_settings_service.rs:110](../../crates/vpn-server/src/services/integration_settings_service.rs#L110)

- 飞书 HTTP 调用与敏感错误隔离。
  [feishu_approval_service.rs:80](../../crates/vpn-server/src/services/feishu_approval_service.rs#L80)

- 管理员鉴权及禁用状态的路由验证。
  [integration_settings_flow.rs:196](../../crates/vpn-server/tests/integration_settings_flow.rs#L196)

- 部署步骤与本地记录的含义。
  [external-api.md:163](../../docs/external-api.md#L163)
