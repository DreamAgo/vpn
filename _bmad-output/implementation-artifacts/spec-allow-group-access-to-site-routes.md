---
title: '允许用户组授权访问站点网关网段'
type: 'bugfix'
created: '2026-08-25'
status: 'done'
baseline_commit: '4186d0b'
context:
  - '{project-root}/_bmad-output/implementation-artifacts/spec-feishu-approval-network-access.md'
  - '{project-root}/_bmad-output/implementation-artifacts/spec-server-managed-peer-routes.md'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** 网关节点已声明 `10.242.101.0/24` 为站点网段，但同一网段无法写入用户组路由，导致受用户组/审批管理的用户无法获得到该站点的客户端路由和服务端 ACL 授权。

**Approach:** 允许用户组的“授权目的网段”与网关节点的“站点源网段”重合；依然由组成员关系或有效审批决定访问权，并由 nftables 按用户 VPN IP 强制执行。

## Boundaries & Constraints

**Always:** 仅取消“用户组目的路由 vs 站点源网段”的互斥校验；保留不同网关站点网段的重叠禁止、VPN 基础网段禁止、默认路由禁止、网关自身网段排除、审批到期和 ACL fail-closed。

**Ask First:** 改变站点主动访问 VPN 客户端的现有放行语义；将站点网段自动授权给未入组或未审批用户；改变超网路由的现有包含语义。

**Never:** 不仅依赖客户端路由作为权限控制；不因站点存在就向所有用户下发；不放开两个网关声明重叠网段；不弱化 nftables 最终 drop 规则。

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|---|---|---|---|
| 精确授权 | 站点与组均为 `10.242.101.0/24` | 允许保存；有权用户获得路由与 ACL | 未授权用户仍被丢弃 |
| 组超网 | 组 `/16` 包含站点 `/24` | 按管理员显式 `/16` 授权，并下发站点 `/24` | 不自动收窄授权 |
| 站点网关重叠 | 另一网关声明相同/包含网段 | 继续拒绝 | 防止 WireGuard AllowedIPs 抢占 |
| 受管用户无授权 | `approval_required` 无组/有效审批 | 仅获得 VPN 基础网段 | nft 最终规则丢弃业务流量 |
| 网关自身注册 | 网关用户的组也包含自有站点 | 不向该网关下发自有网段 | 避免本地 LAN 卷入隧道 |

</frozen-after-approval>

## Code Map

- `crates/vpn-server/src/services/user_group_service.rs` -- 用户组路由写入校验。
- `crates/vpn-server/src/services/peer_service.rs` -- 网关站点路由校验与客户端 `allowed_routes` 计算。
- `crates/vpn-server/src/services/network_acl_service.rs` -- 人工组、审批与 legacy 授权快照。
- `crates/vpn-wireguard/src/acl.rs` -- 按方向渲染站点源和授权目的规则。

## Tasks & Acceptance

**Execution:**
- [x] `user_group_service.rs` -- 允许组路由与活跃站点重合，保留 VPN/默认路由校验并更新单测。
- [x] `peer_service.rs` -- 取消站点写入/重连时的反向组碰撞拒绝，保留网关间碰撞；覆盖注册、心跳和自有网段排除。
- [x] `network_acl_service.rs` -- 允许历史数据中组目的与站点源共存，仍生成有界租约。
- [x] `vpn-wireguard/src/acl.rs` -- 增加同一 CIDR 同时作为站点源和授权目的方向性回归测试。
- [x] 服务层端到端测试 -- 验证人工入组/有效审批获得站点路由，未授权受管用户不获得，legacy 回退不变。

**Acceptance Criteria:**
- Given 网关声明 `10.242.101.0/24` 且用户组授权该网段，when 成员注册或心跳，then 客户端获得该路由且服务端仅放行该成员的 VPN IP。
- Given 另一受管用户无组或有效审批，when 访问该站点，then 不下发路由且 nftables 丢弃流量。
- Given 另一网关声明重叠站点，when 管理员保存，then 仍返回碰撞错误且不改变 WireGuard 配置。

## Spec Change Log

## Design Notes

“站点源”和“授权目的”是同一 CIDR 在不同包方向上的角色，不是地址归属冲突。网关 peer 的 WireGuard AllowedIPs 仍由 `routed_subnets` 唯一决定；用户组只决定客户端选路和 nft ACL lease，不向普通 peer 添加站点所有权。

## Verification

**Commands:**
- `cargo test -p vpn-server user_group_service peer_service network_acl_service` -- 服务语义回归通过。
- `cargo test -p vpn-wireguard acl` -- 双向站点/ACL 规则回归通过。
- `cargo test --workspace` -- 工作区全量测试通过。
- `cargo clippy -p vpn-server -p vpn-wireguard --all-targets -- -D warnings` -- 无警告。
- `cargo fmt --all -- --check && git diff --check` -- 格式与补丁检查通过。

## Suggested Review Order

**路由授权与网关自排除**

- 从客户端实际下发路由入手，理解组授权与站点叠加。
  [`peer_service.rs:510`](../../crates/vpn-server/src/services/peer_service.rs#L510)

- 超网授权通过 CIDR 差集精确排除网关自有 LAN。
  [`peer_service.rs:570`](../../crates/vpn-server/src/services/peer_service.rs#L570)

- 只保留网关之间的站点网段冲突检查。
  [`peer_service.rs:620`](../../crates/vpn-server/src/services/peer_service.rs#L620)

**用户组写入策略**

- 组路由仅禁止 VPN 基础网段，允许站点目的网段。
  [`user_group_service.rs:138`](../../crates/vpn-server/src/services/user_group_service.rs#L138)

- 服务组装仅传入 VPN 子网策略。
  [`main.rs:83`](../../crates/vpn-server/src/main.rs#L83)

**服务端 ACL 强制**

- 从 SQLite 人工组、有效审批与 legacy 事实生成有界租约。
  [`network_acl_service.rs:79`](../../crates/vpn-server/src/services/network_acl_service.rs#L79)

- 验证仅有授权 VPN IP 进入站点目的集合。
  [`network_acl_service.rs:254`](../../crates/vpn-server/src/services/network_acl_service.rs#L254)

- 验证同 CIDR 双向角色共存且保留最终丢弃。
  [`acl.rs:275`](../../crates/vpn-wireguard/src/acl.rs#L275)

**边界回归**

- 创建路径覆盖精确、子网、超网与 VPN 重叠。
  [`user_group_service.rs:225`](../../crates/vpn-server/src/services/user_group_service.rs#L225)

- 更新路径与创建路径保持同一策略。
  [`user_group_service.rs:257`](../../crates/vpn-server/src/services/user_group_service.rs#L257)
