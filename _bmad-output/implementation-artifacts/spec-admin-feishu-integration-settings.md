---
title: '后台管理飞书集成配置'
type: 'feature'
created: '2026-09-11'
status: 'done'
baseline_commit: '15375f0fde8ab36c15e6b5e99c89b21a10e24480'
context:
  - '_bmad-output/implementation-artifacts/spec-client-feishu-login.md'
  - '_bmad-output/implementation-artifacts/spec-feishu-approval-network-access.md'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** 飞书登录与审批配置只能修改容器环境变量，后台看不到启用状态，也无法安全轮换 App Secret；管理员难以判断还有哪些配置必须留在部署层。

**Approach:** 新增独立“集成设置”页面和管理员 API，集中管理飞书登录、审批及外部选项配置；环境变量只在聚合配置尚不存在时初始化一次，保存后重启生效。同步补全配置覆盖清单，明确保留在部署层的启动基础设施和密钥。

## Boundaries & Constraints

**Always:** 飞书登录使用显式开关、App ID、HTTPS 回调地址和 App Secret；审批使用独立开关、审批 Code、三个控件 ID、Verification Token、Encrypt Key，并独立管理至少 32 字符的外部选项 Token。聚合配置单键原子保存并优先于环境变量。所有秘密只允许“保持/替换/显式清除”，GET、响应、日志及审计事件只返回 `*_set`，永不回显秘密。所有飞书变更均标记待重启；启动时只从有效完整快照装配现有不可变服务。严格解析回调 URL：HTTPS、无凭证/fragment/query、路径必须为 `/api/v1/auth/feishu/callback`。审批仍依赖飞书登录、已应用的 kernel 后端及 ACL 前置条件；存在 `approval_required` 用户时禁止通过页面关闭审批。页面防止后台刷新或迟到响应覆盖未保存编辑。

**Ask First:** 热切换 OAuth/provider/审批 worker、迁移受审批用户、改变审批 ACL 语义、把启动基础设施配置改为数据库来源。

**Never:** 不回显、记录或下发 App Secret、Verification Token、Encrypt Key、外部选项 Token；不提供通用 `system_config` 编辑器；不把数据库 URL、数据目录、监听地址、日志级别、JWT/WireGuard 私钥放入页面；不修改 `outputs/`，不自动重启、发布或部署。

## I/O & Edge-Case Matrix

| 场景 | 输入/状态 | 预期行为 | 错误处理 |
|---|---|---|---|
| 首次升级 | DB 无聚合值、环境变量完整 | 原子 seed，运行配置不变 | 非法旧值拒绝启动并指出字段 |
| 查看 | 已配置秘密 | 返回非秘密字段和 `*_set=true` | 非管理员 403 |
| 保存 | 留空秘密/新值/显式清除 | 分别保持/替换/清除，整体原子提交 | 部分配置、弱秘密或非法 URI 返回 400 且不改原值 |
| 启用登录 | 三项完整 | 保存为 desired，提示重启 | 不主动中断在线会话 |
| 关闭审批 | 存在受审批用户 | 不保存、不清理 ACL | 返回明确冲突提示 |

</frozen-after-approval>

## Code Map

- `crates/vpn-api-types/src/system.rs` -- 脱敏视图、更新命令与重启状态契约。
- `crates/vpn-server/src/{config.rs,main.rs,state.rs}` -- 环境种子、启动覆盖及服务装配。
- `crates/vpn-server/src/services/{config_service,integration_settings_service}.rs` -- 聚合持久化、校验、秘密三态和 applied/desired 比较。
- `crates/vpn-server/src/{handlers/system.rs,app.rs}` -- 管理员 GET/PUT API。
- `frontend/src/{pages/IntegrationSettingsPage.tsx,services/auth.ts,types/api.ts,App.tsx,components/layout/AppLayout.tsx}` -- 独立导航、表单和防陈旧响应。
- `docs/{configuration.md,external-api.md}` -- 完整配置覆盖、初始化/重启语义和飞书回调说明。

## Tasks & Acceptance

**Execution:**
- [x] `vpn-api-types`、集成设置服务与仓储 -- 定义单键版本化配置、秘密三态、严格校验、环境一次性 seed 和原子保存。
- [x] `config.rs`、`main.rs`、`state.rs` -- 在数据库初始化后解析有效 desired，保持现有飞书/审批启动顺序和 fail-closed ACL。
- [x] handler/app 与集成测试 -- 增加管理员 GET/PUT，覆盖权限、兼容初始化、原子失败、秘密不泄露及安全禁用。
- [x] 前端页面、路由与类型 -- 增加飞书登录/审批/外部选项区块，秘密输入不 hydration，显式清除需危险确认。
- [x] 文档与延期清单 -- 补齐 44 项配置覆盖；记录通知 URL 泄露、通知非原子更新、TLS 接线及数据库父目录等既有问题。

**Acceptance Criteria:**
- Given 测试服务尚未配置飞书，when 管理员保存完整登录配置并重启，then `/auth/feishu/config` 报告可用且秘密未出现在任何管理响应。
- Given 已有环境变量配置，when 首次升级后再修改环境变量，then DB desired 保持不变；页面轮换秘密并重启后只使用新值。
- Given 任一保存字段非法，when PUT 配置，then 整组不变，运行中的飞书服务和 ACL 不受影响。

## Spec Change Log

## Design Notes

首版坚持“持久化、重启生效”，因为 OAuth provider、审批 worker 和 ACL 生命周期当前在启动时固化。秘密沿用受限 `system_config`/备份的部署信任边界，但 API 只暴露是否已设置；备份必须按敏感凭证保护。配置审计结果中，网络/DNS 与通知已存在页面；监听、数据库、数据目录、日志和系统私钥继续保留在部署层。

## Verification

- `cargo fmt --all -- --check && cargo test --workspace` -- 所有协议、服务及集成测试通过。
- `cargo clippy --workspace --all-targets -- -D warnings` -- 无新增警告。
- `npm --prefix frontend run lint && npm --prefix frontend run build` -- 页面类型与构建通过。
- `cargo check --manifest-path desktop/src-tauri/Cargo.toml && git diff --check` -- 客户端兼容且补丁格式正确。

2026-09-11：以上命令全部通过；前端仅报告既有 chunk 大小提示。另有集成设置 7 项服务单测与管理员 API 流程测试通过；2 项真机测试按预期忽略。

## Review Summary

三路审查发现的审批关闭竞态、非 kernel 后端配置、迟到 GET 覆盖、存量空白值和文档计数问题均已修复，复审无剩余 blocker。多进程 compare-and-set 与秘密数据库加密不属于本次已批准边界，已按单实例部署与现有数据库备份信任模型保留。

## Suggested Review Order

**聚合配置与安全边界**

- 从单键快照、环境种子与重启语义理解整体设计。
  [`integration_settings_service.rs:66`](../../crates/vpn-server/src/services/integration_settings_service.rs#L66)

- 原子校验审批用户并保存，避免关闭 ACL 的竞态。
  [`integration_settings_service.rs:117`](../../crates/vpn-server/src/services/integration_settings_service.rs#L117)

- 严格验证完整性、秘密强度与飞书回调地址。
  [`integration_settings_service.rs:224`](../../crates/vpn-server/src/services/integration_settings_service.rs#L224)

- 启动装配固定 applied 快照，变更只在重启后生效。
  [`main.rs:64`](../../crates/vpn-server/src/main.rs#L64)

**API 与管理页面**

- 管理员接口返回脱敏视图并校验已应用后端。
  [`system.rs:50`](../../crates/vpn-server/src/handlers/system.rs#L50)

- 页面保护未保存输入、迟到响应和危险密钥清除。
  [`IntegrationSettingsPage.tsx:63`](../../frontend/src/pages/IntegrationSettingsPage.tsx#L63)

- 更新契约禁止未知字段，只表达秘密三态操作。
  [`system.rs:450`](../../crates/vpn-api-types/src/system.rs#L450)

**验证与运维边界**

- 集成流程覆盖鉴权、脱敏和失败时整组不变。
  [`integration_settings_flow.rs:166`](../../crates/vpn-server/tests/integration_settings_flow.rs#L166)

- 配置文档明确页面覆盖与仍属部署层的参数。
  [`configuration.md:67`](../../docs/configuration.md#L67)
