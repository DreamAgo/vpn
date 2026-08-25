---
title: '服务端网络参数配置页面'
type: 'feature'
created: '2026-08-25'
status: 'done'
baseline_commit: '92dab4cc9c84a2e68a5726f9f36e8a98ac25301f'
context: []
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** 隧道 MTU 只能通过环境变量和客户端常量调整，不便统一维护；环境变量若持续覆盖还会破坏后台配置。

**Approach:** 新增管理员“网络设置”页面，将 MTU 模式、默认值、最小值、最大值持久化并下发客户端；环境变量仅在数据库无整组配置时初始化一次。

## Boundaries & Constraints

**Always:** 配置包含 `mode`（`fixed`/`auto`）、`default_mtu`、`min_mtu`、`max_mtu`，默认依次为 `fixed`、1360、1280、1420，并满足 `1280 <= min <= default <= max <= 1420`。固定模式使用默认值；自动模式沿用现有传输路径计算并限制在最小/最大值内，无混淆时使用默认值。保存仅对新连接/重连生效。数据库整组原子保存且优先；`VPN_TUN_MTU_MODE/DEFAULT/MIN/MAX` 只作首次种子。协议字段兼容新旧端。

**Ask First:** 改变混淆线协议、强制断开在线节点、修改 `VPN_OBFS_PATH_MTU` 前先确认。

**Never:** 不实现主动 PMTU 探测、自定义分片或 MSS clamp；不暴露密钥；不修改 `outputs/`。

## I/O & Edge-Case Matrix

| 场景 | 状态/输入 | 期望 | 错误处理 |
|---|---|---|---|
| 首次初始化 | DB无值、环境值合法 | 整组一次性落库 | 非法环境值拒绝启动并指出字段 |
| 后续启动 | DB有值、环境值变化 | 保持DB值 | 损坏存量不得静默回退 |
| 管理保存 | 合法策略 | 原子保存并返回有效值 | 非法范围400且原值不变 |
| 客户端注册 | 新客户端 | 下发并应用策略 | 缺字段使用兼容默认值 |
| 权限访问 | 非管理员GET/PUT | 不泄露、不修改 | 403 |

</frozen-after-approval>

## Code Map

- `crates/vpn-server/src/{repositories,services,handlers}` -- 原子初始化、设置服务及管理员API。
- `crates/vpn-server/src/{config.rs,main.rs,state.rs,app.rs}` -- 初始化值、依赖注入与路由。
- `crates/vpn-api-types/src/{system.rs,peer.rs}` -- 设置DTO与兼容下发契约。
- `crates/vpn-server/src/services/peer_service.rs` -- 注册响应携带策略。
- `crates/vpn-cli/src/{daemon.rs,wg_userspace.rs}` -- 验证并应用固定/有界自动MTU。
- `frontend/src/pages/NetworkSettingsPage.tsx`及前端路由、服务、类型 -- 独立管理页面。

## Tasks & Acceptance

**Execution:**
- [x] 实现单键版本化JSON配置、原子首次初始化、校验和管理员GET/PUT。
- [x] 扩展注册下发及客户端MTU选择逻辑，记录模式、范围和最终值。
- [x] 新增网络设置页面，说明生效时机并提供联动校验、保存反馈。
- [x] 补充仓储、服务、API、注册、客户端计算测试及部署文档。

**Acceptance Criteria:**
- Given DB无配置，when 首次启动，then 环境值落库一次且后续重启不被环境变量覆盖。
- Given 管理员保存合法策略，when 页面刷新并有客户端重连，then 页面保留该值且客户端应用它。
- Given 固定模式1360，when 创建隧道，then 各平台TUN MTU为1360。
- Given 自动模式，when 计算MTU，then 结果不越过服务端范围。
- Given 非管理员或非法请求，when 调用API，then 返回403或400且原值不变。

## Spec Change Log

## Design Notes

使用单个 `network_settings_v1` JSON保证四字段原子更新。策略放在注册响应顶层以同时覆盖原生和混淆传输。在线连接不热改，避免接口MTU与启动时缓冲区不一致。

## Verification

**Commands:**
- `cargo fmt --all -- --check`
- `cargo test --workspace`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `npm --prefix frontend run lint && npm --prefix frontend run build`

## Suggested Review Order

**持久化与初始化**

- 从服务入口理解单键初始化、快照与更新边界。
  [`network_settings_service.rs:89`](../../crates/vpn-server/src/services/network_settings_service.rs#L89)

- 启动时结合混淆路径校验并装配共享策略。
  [`main.rs:140`](../../crates/vpn-server/src/main.rs#L140)

**协议与客户端应用**

- 注册响应携带当前策略且不热改在线连接。
  [`peer_service.rs:499`](../../crates/vpn-server/src/services/peer_service.rs#L499)

- fixed/auto 计算后统一执行混淆安全上限保护。
  [`wg_userspace.rs:187`](../../crates/vpn-cli/src/wg_userspace.rs#L187)

**管理界面与 API**

- 页面防止后台刷新和迟到响应覆盖未保存编辑。
  [`NetworkSettingsPage.tsx:12`](../../frontend/src/pages/NetworkSettingsPage.tsx#L12)

- 管理接口统一权限、校验及非法 JSON 的400响应。
  [`system.rs:45`](../../crates/vpn-server/src/handlers/system.rs#L45)

**测试与运维**

- 集成测试覆盖权限、合法更新及失败不改原值。
  [`network_settings_flow.rs:108`](../../crates/vpn-server/tests/network_settings_flow.rs#L108)

- 文档明确一次性环境种子与混淆安全约束。
  [`configuration.md:47`](../../docs/configuration.md#L47)
