---
title: '客户端 DNS 仅保留全局模式'
type: 'refactor'
created: '2026-08-27'
status: 'done'
baseline_commit: 'ba5d6fb62b04e2ad113e449801d91d071039a895'
context: ['_bmad-output/implementation-artifacts/spec-server-managed-dns-settings.md']
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** 客户端按域名分流增加管理和跨平台实现复杂度，用户决定取消该功能。

**Approach:** 仅提供关闭/全局 DNS。启用后系统 DNS 查询统一交给 VPN 网关，服务端继续支持默认上游、最长后缀转发和静态 A 记录；不再配置或下发客户端分流域名。

## Boundaries & Constraints

**Always:** 移除客户端分流执行路径及前端入口。保留断开恢复、失败回滚和产品状态隔离。旧 `split` 模式兼容读取为 `global`，旧域名字段忽略且不再输出；新序列化仅产生 disabled/global，未知模式仍报错。旧环境变量模式 split 兼容全局，分流域名变量不再使用；文档明确这会把系统 DNS 查询统一送往 VPN。保持现有 DNS 网关校验和服务器解析优先级。客户端策略在新连接/重连生效，不强制刷新在线连接。

**Ask First:** 发布或部署、取消服务端按域名选择上游、改用外部 DNS 服务、修改物理网卡 DNS。

**Never:** 不开放公网 53、不修改 VPN 数据流量路由、不写 `/etc/resolv.conf`、不承诺接管应用自带 DoH、不触碰其他 VPN 或 `outputs/`。

## I/O & Edge-Case Matrix

| 场景 | 输入 | 行为 | 错误处理 |
|---|---|---|---|
| 启用 | global + 合法默认上游 | 下发网关全局 DNS，无域名列表 | 非法上游拒绝 |
| 关闭 | disabled | 不下发 DNS，连接生命周期清理本产品状态 | 恢复失败保留错误 |
| 旧配置/旧响应 | split + 历史域名字段 | 兼容解释为 global，忽略域名字段 | 未知模式拒绝 |
| 大量服务端规则 | 合法转发/静态记录列表 | 客户端仍只有固定大小的全局配置 | 不生成逐域名系统命令 |
| 查询到达网关 | 静态/转发/其他域名 | 静态优先、最长后缀、默认上游 | 保持现有故障切换 |

</frozen-after-approval>

## Code Map

- `crates/vpn-api-types/src/{system.rs,peer.rs}` -- 模式、旧字段兼容与 DTO。
- `crates/vpn-server/src/{config.rs,services/network_settings_service.rs,services/peer_service.rs,dns_service.rs}` -- 初始化、持久化、下发及解析回归。
- `crates/vpn-cli/src/daemon.rs`、`crates/vpn-platform/src/dns.rs` -- 客户端校验、系统应用/恢复。
- `frontend/src/{pages/NetworkSettingsPage.tsx,types/api.ts}` -- 全局设置页面及类型。
- `docs/configuration.md` -- 升级及配置行为说明。

## Tasks & Acceptance

**Execution:**
- [x] `crates/vpn-api-types/src/{system.rs,peer.rs}` -- 删除活动 Split 模式和分流字段，增加旧 JSON 兼容与新序列化测试。
- [x] `crates/vpn-server/src/{config.rs,services/network_settings_service.rs,services/peer_service.rs}`、`crates/vpn-server/tests/network_settings_flow.rs` -- 清理分流 seed/下发，保持旧数据库可加载，补保存/注册测试。
- [x] `crates/vpn-cli/src/daemon.rs`、`crates/vpn-platform/src/dns.rs` -- 移除分流校验和命令，保留三平台全局应用/恢复；检查 Windows 不遗留无必要的接口优先级修改。
- [x] `frontend/src/{pages/NetworkSettingsPage.tsx,types/api.ts}` -- 删除分流入口，明确全局 DNS 行为，保留服务端解析配置。
- [x] `crates/vpn-server/src/dns_service.rs`、`docs/configuration.md` -- 保持解析回归，解释旧 split 转全局及重连生效。

**Acceptance Criteria:**
- Given 启用全局 DNS，when 客户端连接或断开，then 仅本产品 DNS 被应用或恢复且不改变流量路由。
- Given 旧 split 数据或响应，when 新版本读取，then 不因已删除字段报错且采用已说明的全局策略。
- Given 管理员配置大量解析规则，when 客户端注册，then DNS 下发与系统命令大小不随规则数量增长。

## Spec Change Log

- 2026-08-27：用户取消自动分流方案，改为仅全局 DNS；旧方案未写入业务代码，已停止。

## Verification

2026-08-27 自动验证全部通过：workspace 单元/集成/doc 测试成功（仅 2 项既有真机测试忽略）；新增旧 JSON/数据库/环境兼容、API 保存、最大 64 条转发 + 256 条静态记录的固定下发及三平台固定命令测试。前端构建仅报告既有 chunk 大小提醒。未操作真实系统 DNS，未发布或部署。

- `cargo test --workspace` -- 协议兼容、保存/注册、DNS 解析和命令生成回归通过。
- `cargo clippy --workspace --all-targets -- -D warnings` -- 无警告。
- `cargo fmt --all -- --check && git diff --check` -- 格式通过。
- `npm --prefix frontend run lint && npm --prefix frontend run build` -- 页面检查通过。
- `cargo check --manifest-path desktop/src-tauri/Cargo.toml` -- GUI 共用客户端兼容。
- 真机限制：本轮不操作系统 DNS、不部署；跨平台实际应用/恢复保留为发布前验证项。

## Review Summary

- 三路独立审查完成；修正全局 DNS 文案，明确默认解析器不覆盖应用 DoH 或其他更具体的系统/VPN DNS 策略，遵守不干预其他 VPN 的边界。
- 既有系统 DNS 命令缺少超时的问题记入 `deferred-work.md`，未扩大本轮实现范围。
- 文案更新后再次通过前端 lint/build 与 diff 格式检查。

## Suggested Review Order

**客户端应用与恢复**

- 固定全局命令，保留产品隔离与失败回滚。
  [dns.rs:57](../../crates/vpn-platform/src/dns.rs#L57)

- 三平台仅生成默认解析策略，不再遍历域名。
  [dns.rs:150](../../crates/vpn-platform/src/dns.rs#L150)

**下发与兼容**

- 仅下发网关与模式，解析规则留在服务端。
  [peer_service.rs:529](../../crates/vpn-server/src/services/peer_service.rs#L529)

**配置与验证**

- 后台只保留关闭和全局，说明生效边界。
  [NetworkSettingsPage.tsx:110](../../frontend/src/pages/NetworkSettingsPage.tsx#L110)

- 旧 split 兼容读取为 global，不再输出旧模式。
  [system.rs:64](../../crates/vpn-api-types/src/system.rs#L64)

- 验证旧 API 配置保存及服务端解析规则保留。
  [network_settings_flow.rs:199](../../crates/vpn-server/tests/network_settings_flow.rs#L199)

- 验证大量规则不会扩大客户端配置。
  [peer_service.rs:1602](../../crates/vpn-server/src/services/peer_service.rs#L1602)

- 说明升级影响及重连要求。
  [configuration.md:58](../../docs/configuration.md#L58)
