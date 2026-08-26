---
title: '服务端管理并下发客户端 DNS'
type: 'feature'
created: '2026-08-26'
status: 'done'
baseline_commit: '053fd3d3a3cf6fefb22ac34e811ad0e084b813da'
context: ['_bmad-output/implementation-artifacts/spec-expand-network-settings-base-config.md']
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** 服务端没有 VPN 内置 DNS，无法集中解析内部记录或转发查询；客户端也不会自动应用、恢复 DNS。

**Approach:** 服务端在 VPN 网关启动受限 DNS 转发器，后台配置默认上游、分流上游和静态 A 记录；客户端按全局/分流策略使用网关 DNS，断开时恢复。

## Boundaries & Constraints

**Always:** DNS 只监听 VPN 网关 UDP/TCP 53、拒绝非 VPN 来源且不发布公网端口。静态 A 优先；最长后缀选择分流上游，其余走默认上游；支持故障切换、TCP 回退和有界缓存。上游仅接受 IP:53 且不能指回自身，域名规范化去重。配置进版本化数据库，环境变量仅首次 seed。客户端应用网关和全局/分流域；失败则连接回滚，断开、重连、异常退出或下次启动只清理本产品状态。新旧版本字段缺省兼容。

**Ask First:** 对公网开放 53、支持动态更新/区域传送、DoH/DoT、非 A 静态记录、修改已连接客户端 DNS、引入独立常驻进程。

**Never:** 不做开放递归/权威 DNS，不永久覆盖原 DNS；Linux 不改 `/etc/resolv.conf`；不删其他 VPN 状态；不记录完整查询名；不修改 `outputs/`。

## I/O & Edge-Case Matrix

| 场景 | 输入 / 状态 | 期望行为 | 失败处理 |
|---|---|---|---|
| 静态命中 | 主机名 → IPv4 | 直接返回 A 记录 | 非 A 查询按规则转发 |
| 分流转发 | 后缀 → 上游组 | 最长后缀上游成功响应 | 超时切换，全部失败 SERVFAIL |
| 客户端全局/分流 | 网关 DNS + 可选后缀 | 全部或仅匹配域查询网关 | 应用失败连接回滚 |
| 重连/恢复 | 已有本产品状态 | 清理后重建 | 不触碰其他产品 |

</frozen-after-approval>

## Code Map

- `crates/vpn-api-types/src/{system.rs,peer.rs}` -- DNS DTO、校验和注册契约。
- `crates/vpn-server/src/{config.rs,dns_service.rs,services/network_settings_service.rs,services/peer_service.rs}` -- v3 迁移、解析器、热配置与下发。
- `frontend/src/{pages/NetworkSettingsPage.tsx,types/api.ts}` -- DNS 表单。
- `crates/vpn-platform/src/dns.rs` -- 三平台应用、所有权和恢复。
- `crates/vpn-cli/src/{daemon.rs,wg_userspace.rs}`、`desktop/src-tauri/src/manager.rs` -- 生命周期与回滚。

## Tasks & Acceptance

**Execution:**
- [x] 扩展 v3 配置/API，迁移 v2/环境 seed；添加模式、上游、分流规则和静态 A 记录表单。
- [x] 实现仅 VPN 可达的 UDP/TCP DNS 转发器、最长后缀路由、缓存、故障切换和热配置。
- [x] 实现三平台 DNS 会话，接入连接、失败和退出清理。
- [x] 测试协议、隔离、转发、缓存、迁移、命令生成及回滚；更新文档。

**Acceptance Criteria:**
- Given 保存合法规则，when VPN 客户端查询，then 静态/分流/默认优先级正确且公网不能访问 53。
- Given 客户端重连，when DNS 下发，then 全局或分流系统策略生效。
- Given 应用失败或断开，when 清理结束，then 原 DNS 恢复且不显示假连接。
- Given 新旧版本或其他 VPN 共存，when 连接/断开，then 字段兼容且不删除他方状态。

## Spec Change Log

## Verification

自动检查已通过：Rust workspace 全量测试、clippy `-D warnings`、格式与 diff 检查、前端 lint/build。真机 DNS 应用/恢复和公网 53 隔离保留为发布测试环境验证项。

**Commands:**
- `cargo fmt --all -- --check && cargo test --workspace`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `npm --prefix frontend run lint && npm --prefix frontend run build`
- `git diff --check`

**Manual checks:**
- VPN 内验证解析/TCP 回退，公网确认 53 不监听；三平台验证应用和恢复。

## Suggested Review Order

**服务端解析边界**

- 从受限监听、来源校验到转发缓存，先掌握核心安全模型。
  [`dns_service.rs:95`](../../crates/vpn-server/src/dns_service.rs#L95)

- 集中规范化配置并拒绝回环、非法上游及重叠规则。
  [`system.rs:88`](../../crates/vpn-api-types/src/system.rs#L88)

**配置生命周期**

- 数据库配置热更新共享快照，环境变量只承担首次初始化。
  [`network_settings_service.rs:125`](../../crates/vpn-server/src/services/network_settings_service.rs#L125)

- 后台表单完整绑定模式、上游、分流规则和静态记录。
  [`NetworkSettingsPage.tsx:118`](../../frontend/src/pages/NetworkSettingsPage.tsx#L118)

**客户端应用与恢复**

- 平台适配只管理本产品 DNS 状态并提供幂等恢复。
  [`dns.rs:31`](../../crates/vpn-platform/src/dns.rs#L31)

- DNS 应用失败与隧道生命周期统一回滚，避免假连接。
  [`wg_userspace.rs:472`](../../crates/vpn-cli/src/wg_userspace.rs#L472)

**验证支撑**

- 覆盖迁移、热更新和默认禁用的兼容路径。
  [`network_settings_service.rs:598`](../../crates/vpn-server/src/services/network_settings_service.rs#L598)
