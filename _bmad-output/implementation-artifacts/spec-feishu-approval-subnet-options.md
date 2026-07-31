---
title: '飞书审批外部选项：网段列表'
type: 'feature'
created: '2026-07-31'
status: 'done'
baseline_commit: '5cd1356f6c25ef5ed89e22c5c64d132d6f432c9e'
context:
  - '{project-root}/docs/external-api.md'
  - '{project-root}/docs/configuration.md'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** 飞书审批单选/多选控件无法读取易链的动态网段，管理员只能重复维护静态选项；其他目录数据后续也会遇到相同问题。

**Approach:** 增加符合飞书“关联外部选项”明文契约的统一入口，以稳定的数据源标识分发给可扩展 provider；首个 `subnets` provider 从现有网段目录生成选项，并支持搜索和游标分页。

## Boundaries & Constraints

**Always:** 使用 `POST /api/v1/integrations/feishu/approval-options/{source}`；字段严格兼容飞书契约；用独立环境变量 `VPN_FEISHU_APPROVAL_OPTIONS_TOKEN` 校验请求体 `token`，且不记录 token；选项 `id` 使用网段 ID，文案包含名称和 CIDR；返回默认 `zh_cn` 资源；服务端生成并校验游标，单页不少于 10 项；未知 source 和非法游标返回确定性错误。

**Ask First:** 改网段表结构、既有管理 API、引入 Key 加密或持久化 webhook token 前先征得用户同意。

**Never:** 不复用 OAuth Secret、JWT 或 API Key；不经过 JWT 管理端认证链；本次不做 Key/AES-CBC、按用户过滤或管理 UI。

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|---------------|----------------------------|----------------|
| 获取首屏 | `source=subnets`、token 正确 | 返回稳定排序的网段选项与 `zh_cn` 文案 | N/A |
| 搜索与分页 | `query` 匹配名称或 CIDR，或携带服务端游标 | 返回对应页；有后页时返回新游标 | 空结果成功返回空数组 |
| 身份校验失败 | token 缺失或错误 | 不查询网段、不泄露配置 | HTTP 401，飞书格式非零 `code` |
| 非法请求 | 未知 `source` 或伪造/越界游标 | 不回退到其他数据源 | HTTP 400/404，飞书格式非零 `code` |
| 未配置 | 服务端未设置专用 token | 接口保持不可用 | HTTP 503，且日志不包含敏感值 |

</frozen-after-approval>

## Code Map

- `crates/vpn-api-types/src/external_options.rs` -- 飞书外部选项请求、响应、选项和国际化 DTO。
- `crates/vpn-server/src/services/external_options_service.rs` -- provider 抽象、注册/分发、token、游标与分页。
- `crates/vpn-server/src/handlers/external_options.rs` -- 飞书专用 HTTP 状态与响应封装。
- `crates/vpn-server/src/app.rs` -- 在公开路由区注册动态 source 路由，不经过 JWT 中间件。
- `crates/vpn-server/src/config.rs`、`crates/vpn-server/src/main.rs`、`crates/vpn-server/src/state.rs` -- 加载 token 并装配服务。
- `crates/vpn-server/tests/external_options_flow.rs` -- 端到端覆盖认证、网段映射、搜索、分页和错误分支。
- `crates/vpn-server/src/handlers/openapi.rs`、`docs/external-api.md`、`docs/configuration.md` -- 发布契约、配置和限制。

## Tasks & Acceptance

**Execution:**
- [x] `crates/vpn-api-types/src/external_options.rs`、`crates/vpn-api-types/src/lib.rs` -- 增加独立于 `ApiResponse` 的飞书 DTO，锁定字段结构。
- [x] `crates/vpn-server/src/services/external_options_service.rs`、`crates/vpn-server/src/services/mod.rs` -- 定义 provider 与 `subnets` 实现，集中处理认证、搜索和游标。
- [x] `crates/vpn-server/src/handlers/external_options.rs`、`crates/vpn-server/src/handlers.rs`、`crates/vpn-server/src/app.rs` -- 暴露公开 webhook 路由，并将所有成功/失败映射为飞书响应结构。
- [x] `crates/vpn-server/src/config.rs`、`crates/vpn-server/src/main.rs`、`crates/vpn-server/src/state.rs` -- 注入专用 token；空白值视为未配置。
- [x] `crates/vpn-server/tests/external_options_flow.rs` -- 用内存 SQLite 和 Router 验证 I/O 矩阵及稳定 ID/文案。
- [x] `crates/vpn-server/src/handlers/openapi.rs`、`docs/external-api.md`、`docs/configuration.md` -- 记录外部契约、环境变量、飞书后台填写方式和明文传输限制。

**Acceptance Criteria:**
- Given 已配置 token 且有网段，when 飞书请求 `subnets`，then 在 3 秒预算内返回可显示且 ID 稳定的选项。
- Given 新增另一个外部选项来源，when 实现并注册新的 provider，then 无需复制 token 校验、游标解析或 HTTP 响应逻辑。
- Given 现有 admin 网段 API 客户端，when 本功能上线，then 原路径、认证方式与响应结构保持不变。
- Given OpenAPI 与部署文档，when 运维配置审批控件，then 能找到 URL、token 和首版不支持 Key 的说明。

## Spec Change Log

## Design Notes

飞书协议不同于项目 `ApiResponse`：它使用 `msg` 和 `data.result`，因此 DTO 与错误封装留在集成边界。handler 只处理 `{source}` 和标准请求，provider 返回中立 `{id,label,is_default}`，service 统一生成 i18n，后续数据源无需复制协议逻辑。

`page_token` 编码版本、source、query 指纹和 offset，拒绝跨源/跨查询复用；固定 50 项。首版明文返回，飞书后台 Key 留空。

## Verification

**Commands:**
- `cargo fmt --all -- --check` -- 新增 Rust 代码格式正确。
- `cargo test -p vpn-api-types` -- 飞书 DTO 字段名与结构序列化测试通过。
- `cargo test -p vpn-server external_options` -- provider、配置与 HTTP 端到端测试通过。
- `cargo test -p vpn-server` -- 既有服务端行为无回归。
- `cargo clippy -p vpn-server -p vpn-api-types --all-targets -- -D warnings` -- 无新增 lint。

## Suggested Review Order

**入口与协议边界**

- 公开动态 source 路由独立于 JWT 管理端认证链。
  [`app.rs:68`](../../crates/vpn-server/src/app.rs#L68)

- 统一映射解析、超时和业务错误为飞书信封。
  [`external_options.rs:13`](../../crates/vpn-server/src/handlers/external_options.rs#L13)

**扩展模型与安全**

- Provider 注册将新目录接入收敛为单一扩展点。
  [`external_options_service.rs:25`](../../crates/vpn-server/src/services/external_options_service.rs#L25)

- 认证、搜索、分页和 i18n 包装集中复用。
  [`external_options_service.rs:94`](../../crates/vpn-server/src/services/external_options_service.rs#L94)

- HMAC 签名游标绑定 source、query 与 offset。
  [`external_options_service.rs:211`](../../crates/vpn-server/src/services/external_options_service.rs#L211)

**装配与配置**

- 专用 token 脱敏并强制最小长度。
  [`config.rs:69`](../../crates/vpn-server/src/config.rs#L69)

- 启动时注册首个 subnets provider。
  [`main.rs:79`](../../crates/vpn-server/src/main.rs#L79)

**契约、测试与运维**

- 第三方 DTO 保持与项目 ApiResponse 隔离。
  [`external_options.rs:10`](../../crates/vpn-api-types/src/external_options.rs#L10)

- 端到端覆盖成功、分页、认证和解析失败。
  [`external_options_flow.rs:84`](../../crates/vpn-server/tests/external_options_flow.rs#L84)

- 运维文档给出飞书后台配置与明文限制。
  [`external-api.md:91`](../../docs/external-api.md#L91)
