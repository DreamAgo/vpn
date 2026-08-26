---
title: '补齐网络设置页的虚拟子网与数据面配置'
type: 'feature'
created: '2026-08-25'
status: 'done'
baseline_commit: '78ebe604a1be7fcfef6de8bac5a2ca813d82d02a'
context: ['docs/configuration.md']
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** 当前“网络设置”只管理 MTU，虚拟子网、WireGuard 与混淆参数仍依赖环境变量，且页面无法区分当前运行值和待重启值。

**Approach:** 将非敏感数据面参数纳入版本化数据库配置，把页面扩为基础 VPN、UDP 混淆、LAN 路由、MTU 四区；环境变量只做首次初始化，启动型修改保存后由管理员自行重启。

## Boundaries & Constraints

**Always:** 管理 `vpn_subnet/vpn_listen_port/vpn_endpoint/wg_backend/wg_interface`、混淆 `enabled/mode/bind_addr/public_endpoint/path_mtu`、MTU 和 `server_routes`。API 返回运行值、保存值、`restart_required/psk_configured`。LAN 路由热更新，MTU 对重连生效，其余重启生效。首次迁移 v1 MTU，其余由环境变量 seed；之后 DB 优先。整组校验、原子保存，坏数据拒绝启动。虚拟子网须容纳服务端和客户端；改变时若 `peers` 有任意记录则拒绝，启动也拒绝子网外 Peer IP。宽泛 LAN/组路由仍可包含 VPN 子网。

**Ask First:** 自动迁移/重编号 Peer IP、清空节点、自动重启、强制断线或改变混淆线协议。

**Never:** 不管理 HTTP/HTTPS、域名、数据库、数据目录等控制面配置；不返回或记录 WG 私钥、混淆 PSK。PSK 仍由秘密环境变量提供。不得修改 `outputs/`。

## I/O & Edge-Case Matrix

| 场景 | 状态 | 期望 | 失败处理 |
|---|---|---|---|
| 首次升级 | 旧 MTU + 环境变量 | 迁移并 seed v2 | 非法字段拒绝启动 |
| 保存启动型字段 | 合法配置 | 保存、运行值不变、提示重启 | 非法则整组不变 |
| 改虚拟子网 | 存在 Peer | 不保存 | 400 提示彻底清理节点 |
| 改 LAN 路由 | 合法 CIDR/空列表 | 保存并刷新 ACL/路由 | 非法不改变配置 |
| 启用混淆 | 前置条件齐全 | 与 MTU 校验后保存 | 缺条件则拒绝 |

</frozen-after-approval>

## Code Map

- `crates/vpn-server/src/{config.rs,main.rs,startup.rs,services/network_settings_service.rs,repositories/peer_repo_sqlite.rs}` -- 迁移、装配与子网保护。
- `crates/vpn-api-types/src/system.rs`、`crates/vpn-server/src/{handlers/system.rs,state.rs,app.rs}` -- 脱敏 DTO 与 API。
- `frontend/src/pages/NetworkSettingsPage.tsx`、`services/auth.ts`、`types/api.ts` -- 四分区页面；`docs/` 更新运维说明。

## Tasks & Acceptance

**Execution:**
- [x] 实现 v2 聚合配置、迁移、seed、快照、校验，并驱动 IpPool、WG、ACL 和混淆代理。
- [x] 扩展脱敏 API 与四分区页面，保留 LAN 热更新、待重启提示和编辑保护。
- [x] 测试迁移、DB 优先、原子失败、权限、Peer 子网和启动装配；更新文档。

**Acceptance Criteria:**
- Given 已部署实例升级，when 启动，then 完整配置与运行值一致，后续非敏感环境变量不覆盖 DB。
- Given 空 Peer 库改子网并重启，when 节点注册，then 地址从新子网正确分配；有 Peer 时保存失败且配置不变。
- Given 保存启动型字段，when 重启前后查看，then 分别显示待重启和已生效；非管理员为 403，API/日志无秘密泄露。

## Spec Change Log

## Design Notes

单个 v2 JSON 保证原子性；`server_routes` 延用热更新存储。保存启动型字段只更新 desired，applied 保持到重启。

## Verification

- `cargo fmt --all -- --check && cargo test --workspace`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `npm --prefix frontend run lint && npm --prefix frontend run build`
- `git diff --check`

## Suggested Review Order

**持久化与启动装配**

- 从统一入口理解 v2 迁移、DB 优先和运行快照。
  [`network_settings_service.rs:60`](../../crates/vpn-server/src/services/network_settings_service.rs#L60)

- 启动只用数据库有效值装配 IpPool、WG、ACL 与混淆代理。
  [`main.rs:64`](../../crates/vpn-server/src/main.rs#L64)

- LAN 路由首次 seed，后续严格以数据库为准。
  [`network_settings_service.rs:337`](../../crates/vpn-server/src/services/network_settings_service.rs#L337)

**安全更新与生效边界**

- 整组校验、事务保存、MTU 热切换和注册闸门在同一临界区。
  [`network_settings_service.rs:154`](../../crates/vpn-server/src/services/network_settings_service.rs#L154)

- 待重启子网阻止旧地址池继续创建或恢复节点。
  [`peer_service.rs:347`](../../crates/vpn-server/src/services/peer_service.rs#L347)

- 管理 API 串行读取快照、保存路由并刷新运行状态。
  [`system.rs:50`](../../crates/vpn-server/src/handlers/system.rs#L50)

**契约与管理页面**

- 脱敏 applied/desired 契约集中定义校验和重启状态。
  [`system.rs:71`](../../crates/vpn-api-types/src/system.rs#L71)

- 四分区页面展示待重启字段并保留异步编辑保护。
  [`NetworkSettingsPage.tsx:13`](../../frontend/src/pages/NetworkSettingsPage.tsx#L13)

**回归测试**

- 迁移、路由 DB 优先、宽泛路由与子网闸门集中验证。
  [`network_settings_service.rs:429`](../../crates/vpn-server/src/services/network_settings_service.rs#L429)

- Peer 层确认待重启期间不会写入旧子网 IP。
  [`peer_service.rs:1409`](../../crates/vpn-server/src/services/peer_service.rs#L1409)

- API 层覆盖管理员权限、原子失败与 applied MTU。
  [`network_settings_flow.rs:112`](../../crates/vpn-server/tests/network_settings_flow.rs#L112)
