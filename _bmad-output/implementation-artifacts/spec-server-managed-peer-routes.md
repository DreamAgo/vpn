---
title: '站点网关路由仅由服务端管理'
type: 'feature'
created: '2026-07-17'
status: 'in-review'
baseline_commit: 'a01aea0e4cd4f19e65100265dfd14c29193de343'
context: []
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** 客户端目前可在注册时写入 `routed_subnets`，并会在重连时用空列表清除后台配置，导致网关重启后站点内网不可达。

**Approach:** 将 peer 承载网段的唯一写入口收敛到 admin API。注册只更新设备元数据；新 peer 默认无网段，既有 peer 重连保留后台配置，旧客户端携带该字段时也忽略。

## Boundaries & Constraints

**Always:** `routed_subnets` 只能由 `/api/v1/admin/peers/{id}` 修改；注册、重连、桌面端和 CLI 均不得改写；新 peer 初始为空；管理员仍可配置、替换或清空多个 CIDR；继续使用现有碰撞校验、WireGuard allowed-ips 和心跳动态下发。

**Ask First:** 数据库迁移、改变 admin API、删除现有路由数据，或线上连接中断超过一次容器重启。

**Never:** 不保留客户端 `--route`、路由凭证或环境变量入口；不以 `0.0.0.0/0` 代替通用转发；不改变用户组路由和服务端自身 LAN 路由。

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| 新节点注册 | 新旧客户端注册 | 创建 peer，网段为空 | 携带 CIDR 也不生效 |
| 网关重连 | 后台已有 `192.168.186.0/24` | 更新设备信息并保留网段 | 不删除路由 |
| 管理员编辑 | 设置、替换或清空网段 | 校验、持久化并同步路由 | 沿用现有校验错误 |
| 旧客户端声明 | 请求携带 `routed_subnets` | 兼容接收但忽略 | 不修改数据库 |

</frozen-after-approval>

## Code Map

- `crates/vpn-api-types/src/peer.rs` -- 移除注册请求中的客户端路由字段，保留 admin DTO。
- `crates/vpn-server/src/services/peer_service.rs`、`repositories/peer_repo_sqlite.rs` -- 新节点写空；重注册 SQL 保留后台路由。
- `crates/vpn-cli/src/{cli.rs,config.rs,daemon.rs,main.rs}` -- 移除 `--route`、路由凭证和上报链路。
- `desktop/src-tauri/src/{commands.rs,manager.rs}` -- 移除路由读取与传递。
- CLI/API/服务端测试与客户端文档 -- 覆盖新权限边界并更新运维说明。

## Tasks & Acceptance

**Execution:**
- [x] 更新共享 DTO、服务端注册逻辑和仓储 SQL，强制仅 admin 可写。
- [x] 清理 CLI/桌面端的声明参数、凭证和调用链。
- [x] 补测试：旧字段忽略、重连保留、新 peer 为空、admin 可修改/清空。
- [x] 更新文档并完成服务端、CLI、桌面端和管理后台本地构建验证。
- [ ] 构建镜像并保留数据卷滚动替换线上容器，完成真机复测。

**Acceptance Criteria:**
- Given `szjx` 已配置 `192.168.186.0/24`，when 客户端重启重连，then 网段仍保留且继续下发。
- Given 非管理员客户端，when 新旧协议尝试声明网段，then 服务端不写入。
- Given 管理员修改或清空网段，when 更新成功，then数据库、内核与在线节点按现有机制同步。

## Spec Change Log

## Design Notes

从注册 DTO 删除字段后，Serde 默认忽略旧客户端的未知字段，可兼容旧版本并立即收回权限。数据库列和 admin DTO 保留；`update_registration` 只更新设备元数据。显式清空仅允许 admin PATCH 空数组。

## Verification

**Commands:**
- `cargo test -p vpn-api-types -p vpn-cli -p vpn-server`
- `cargo clippy -p vpn-api-types -p vpn-cli -p vpn-server --all-targets -- -D warnings`
- 桌面端类型检查；线上重启容器并复测 `szjx` 路由保留和内网连通。

## Suggested Review Order

**服务端权限边界**

- 注册只继承管理员配置，并隔离槽位接管与并发更新。
  [`peer_service.rs:254`](../../crates/vpn-server/src/services/peer_service.rs#L254)

- 重注册 SQL 原子保留或由服务端清空路由。
  [`peer_repo_sqlite.rs:199`](../../crates/vpn-server/src/repositories/peer_repo_sqlite.rs#L199)

- 唯一管理入口继续负责校验、持久化和数据面同步。
  [`peer_service.rs:916`](../../crates/vpn-server/src/services/peer_service.rs#L916)

**客户端能力收回**

- 注册契约删除客户端可写网段，兼容忽略旧字段。
  [`peer.rs:12`](../../crates/vpn-api-types/src/peer.rs#L12)

- CLI 明确拒绝已删除的 `--route` 参数。
  [`cli.rs:254`](../../crates/vpn-cli/src/cli.rs#L254)

- 登录与注销清除旧版本残留路由凭证。
  [`config.rs:20`](../../crates/vpn-cli/src/config.rs#L20)

- 管理后台指南改为节点连接后由管理员授权。
  [`ConnectionGuidePage.tsx:263`](../../frontend/src/pages/ConnectionGuidePage.tsx#L263)

**边界回归**

- 新设备接管槽位时必须清空旧网关路由。
  [`peer_service.rs:1437`](../../crates/vpn-server/src/services/peer_service.rs#L1437)

- 强制下线节点不得抢回已转授给其他节点的网段。
  [`peer_service.rs:2355`](../../crates/vpn-server/src/services/peer_service.rs#L2355)
