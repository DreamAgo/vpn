---
title: '允许用户组路由包含 VPN 基础网段'
type: 'bugfix'
created: '2026-08-25'
status: 'done'
baseline_commit: '7024d0a'
context:
  - '{project-root}/_bmad-output/implementation-artifacts/spec-allow-group-access-to-site-routes.md'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** 测试环境 VPN 基础网段为 `10.9.0.0/24`，当管理员用 `10.0.0.0/8` 授权整个业务地址段时，用户组校验因为两者重叠而拒绝保存，尽管客户端路由可以依靠最长前缀匹配安全共存。

**Approach:** 将用户组策略改为非对称包含判定：拒绝等于 VPN 网段或落在 VPN 网段内的路由，允许严格包含 VPN 网段的超网。

## Boundaries & Constraints

**Always:** 保持 `0.0.0.0/0` 禁止；保持 VPN 基础网段本身及其内部子网禁止；保持用户组/审批授权、nftables 按 VPN IP 放行和最终 drop 规则。

**Ask First:** 改变 VPN 节点之间的固定放行语义；为客户端增加本地 LAN 冲突自动解决；允许默认路由。

**Never:** 不将超网自动收窄为站点网段；不把超网自动授权给未入组或无有效审批用户；不弱化 ACL fail-closed。

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|---|---|---|---|
| VPN 超网 | VPN=`10.9.0.0/24`，组路由=`10.0.0.0/8` | 允许创建/更新，授权用户获得 `/8` 路由与 ACL lease | N/A |
| VPN 精确网段 | 组路由=`10.9.0.0/24` | 拒绝 | 错误包含冲突路由和 VPN 网段 |
| VPN 内部子网 | 组路由=`10.9.0.128/25` 或 `10.9.0.3/32` | 拒绝 | 不保存任何部分更新 |
| 默认路由 | 组路由=`0.0.0.0/0` | 继续拒绝 | 保留现有全隧道提示 |
| 不相交网段 | 组路由=`10.242.101.0/24` | 继续允许 | N/A |

</frozen-after-approval>

## Code Map

- `crates/vpn-server/src/services/user_group_service.rs` -- 用户组路由归一化与 VPN 子网边界校验。
- `crates/vpn-server/src/services/peer_service.rs` -- 客户端 `allowed_routes` 计算，已支持超网与更具体 VPN 路由共存。
- `crates/vpn-server/src/services/network_acl_service.rs` -- 将组路由生成按源 VPN IP 限定的 ACL lease。
- `crates/vpn-wireguard/src/acl.rs` -- 渲染目的超网授权与最终 drop。

## Tasks & Acceptance

**Execution:**
- [x] `crates/vpn-server/src/services/user_group_service.rs` -- 改为只拒绝 `vpn_subnet.contains(group_route)`，并在错误中标出具体网段。
- [x] `crates/vpn-server/src/services/user_group_service.rs` -- 覆盖 create/update 的超网、精确、子网、主机路由、默认路由和普通站点回归。
- [x] `crates/vpn-server/src/services/network_acl_service.rs` 与 `crates/vpn-wireguard/src/acl.rs` -- 增加 VPN 超网租约及 fail-closed 回归断言，不改生产规则。

**Acceptance Criteria:**
- Given VPN 基础网段是 `10.9.0.0/24`，when 管理员创建或更新用户组路由为 `10.0.0.0/8`，then 保存成功且授权快照仅包含有权 VPN IP。
- Given 同一 VPN 网段，when 路由等于或位于 `10.9.0.0/24` 内，then 返回明确验证错误且不持久化。

## Spec Change Log

## Design Notes

IPv4 CIDR 重叠关系只可能是一方包含另一方。对用户组路由 `group_route`，校验应使用 `vpn_subnet.contains(group_route)`，而不是对称 overlap。`group_route.contains(vpn_subnet)` 是可允许的超网，客户端与内核均由更具体的 VPN `/24` 获得优先级。

## Verification

**Commands:**
- `cargo test -p vpn-server user_group_service` -- 用户组路由边界矩阵通过。
- `cargo test -p vpn-server network_acl_service` -- 授权快照通过。
- `cargo test -p vpn-wireguard acl` -- 超网 ACL 与最终 drop 通过。
- `cargo test --workspace` -- 工作区全量回归通过。
- `cargo clippy -p vpn-server -p vpn-wireguard --all-targets -- -D warnings` -- 无警告。
- `cargo fmt --all -- --check && git diff --check` -- 格式与补丁检查通过。

## Suggested Review Order

**VPN 超网校验**

- 从非对称包含判定入手，理解超网放行边界。
  [`user_group_service.rs:138`](../../crates/vpn-server/src/services/user_group_service.rs#L138)

- 创建路径覆盖超网、精确、子网与混合输入。
  [`user_group_service.rs:226`](../../crates/vpn-server/src/services/user_group_service.rs#L226)

- 更新失败时保留原名称和完整路由集合。
  [`user_group_service.rs:296`](../../crates/vpn-server/src/services/user_group_service.rs#L296)

- 独立的 `10.9.0.0/24` 策略防止测试假阳性。
  [`user_group_service.rs:359`](../../crates/vpn-server/src/services/user_group_service.rs#L359)

**下发与授权强制**

- 注册与心跳同时保留 VPN `/24` 和授权 `/8`。
  [`peer_service.rs:2394`](../../crates/vpn-server/src/services/peer_service.rs#L2394)

- 数据库快照精确验证 `/8` 的授权源 IP 集合。
  [`network_acl_service.rs:275`](../../crates/vpn-server/src/services/network_acl_service.rs#L275)

- nft 规则必须绑定源集合并保留最终 drop。
  [`acl.rs:299`](../../crates/vpn-wireguard/src/acl.rs#L299)
