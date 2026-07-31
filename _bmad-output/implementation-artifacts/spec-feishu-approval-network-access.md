---
title: '飞书审批开通与延期网络访问（首期）'
type: 'feature'
created: '2026-07-31'
status: 'done'
baseline_commit: 'fc413270af50300b396e5e912ed127df22ec533b'
context:
  - '{project-root}/_bmad-output/implementation-artifacts/spec-client-feishu-login.md'
  - '{project-root}/_bmad-output/implementation-artifacts/spec-feishu-approval-subnet-options.md'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** 飞书登录账号没有网络授权到期时间；无组用户还会回退获得全局路由，无法通过“网络授权申请”安全开通或延期。

**Approach:** 审批表“网络组”选择一个易链用户组。审批通过后按申请人、用户组和到期日记录授权；新飞书账号进入审批管控。有效授权同时驱动客户端路由和当前生产 Docker/kernel WireGuard 的 nftables ACL，到期自动停止业务访问。历史飞书与密码账号保持原行为。

## Boundaries & Constraints

**Always:** 只处理配置的 `approval_code` 且实例详情再次为 `APPROVED`；按稳定控件 ID 与 `user_group.id` 授权；到期日保存为上海时区次日 00:00 的独占 `expires_at`；审批实例幂等且互不缩短；人工组与有效审批组取并集；新飞书账号为 `approval_required`，无有效组时只允许 VPN 基础网段；历史账号为 `legacy`；服务端按 peer VPN IP 强制 ACL；事件验签、解密、验 token、持久化后快速 ACK，敏感数据不入日志。

**Ask First:** 审批撤回后追溯撤权；迁移历史飞书账号；单张审批选择多组；允许默认路由、VPN 子网或站点源网段作为普通组路由；扩展到 userspace/auto 或非 Docker 部署。

**Never:** 仅靠客户端路由充当授权；审批覆盖/删除人工组；按中文字段名、邮箱模糊匹配或选项文案授权；未验证事件直接写库；ACL 故障时静默放行。

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|---|---|---|---|
| 首次开通 | 新申请人、合法组、未来日期 | 原子创建受限账号、绑定身份和授权；ACL 生效 | 失败不留半成账号/授权 |
| 延期/并存 | 已有账号、多个审批 | 分别保存 `expires_at`，任一有效授权即可继续 | 不覆盖或缩短旧授权 |
| 到期 | `now >= expires_at` | nft timeout 与快照撤销该组 | 不依赖客户端诚信或进程持续存活 |
| 重放/篡改 | 重复 uuid/instance 或载荷变化 | 相同事件幂等 ACK；不同载荷拒绝告警 | 避免重复授权及重推风暴 |
| ACL 不可用 | nft 缺失或规则失败 | 生产 kernel 后端拒绝启动/运行时 fail-closed 重试 | 不退化为软路由控制 |

</frozen-after-approval>

## Code Map

- `migrations/20260731163000_feishu_access_grants.sql`、`crates/vpn-server/src/repositories/access_grant_repo_sqlite.rs` -- 账号模式、事件 inbox、逐审批到期授权。
- `crates/vpn-server/src/services/feishu_approval_service.rs`、`crates/vpn-server/src/handlers/feishu_approval.rs` -- 安全 webhook、实例查询、身份绑定/建号与异步处理。
- `crates/vpn-wireguard/src/acl.rs`、`crates/vpn-server/src/services/network_acl_service.rs` -- 当前 Docker/kernel 环境的原子 nft ACL 与到期租约。
- `crates/vpn-server/src/services/{external_options_service,peer_service}.rs`、`crates/vpn-server/src/{config.rs,state.rs,main.rs,app.rs}`、`docker/*` -- 用户组选项、路由语义、装配和运行依赖。

## Tasks & Acceptance

**Execution:**
- [x] migration/repositories/API DTO -- 增加 `access_mode`、durable inbox、逐审批 `group_id/expires_at/reason`，并在单事务中完成身份绑定、建号和授权。
- [x] external options/config -- 注册 `user_groups`，配置 approval code、三个控件 ID、Verification Token 与 Encrypt Key，保持密钥脱敏。
- [x] approval handler/service -- 实现 challenge、签名、解密、token/时间窗/uuid 幂等、快速 ACK、tenant token 和实例 worker；文档说明手动订阅。
- [x] identity/routes -- 新飞书原生账号使用 `approval_required`；历史/邮箱绑定账号保持 `legacy`；人工组与未过期审批组求并集。
- [x] nft ACL/Docker -- 原子更新独立 nft table，使用短租约及精确 timeout；接入启动、peer、组和审批变更，失败时 fail-closed。
- [x] tests/docs -- 覆盖矩阵、日期边界、并发重放、ACL 规则生成；目标 Docker 真机审批前/后/到期数据包验证保留为部署前手工检查。

**Acceptance Criteria:**
- Given 新飞书用户无审批，when 登录、注册或手工添加路由，then 仅 VPN 基础网段可达，业务流量被服务器丢弃。
- Given 合法审批通过，when worker 完成，then 账号创建或延期，数据库记录到期时间，客户端路由与服务端 ACL 同步生效。
- Given 授权到期或服务进程异常，when 到达独占到期/租约上限，then 对应访问停止，其他人工或未到期授权不受影响。
- Given 现有密码或历史飞书用户，when 上线，then 既有未分组回退和访问保持兼容。

## Spec Change Log

## Design Notes

SQLite 是事实源，nft 是派生状态。独立 `inet` table 在 forward priority `-100` 只处理 `iifname=wg0`：放行基础通信、合法站点源及 `/32 → 有效组路由`，其余丢弃；不改 INPUT、OUTPUT 和 NAT。规则校验后原子替换，精确 timeout 与短租约保证故障后 fail-closed。

## Verification

**Commands:**
- `cargo fmt --all -- --check && cargo test --workspace`
- `cargo clippy -p vpn-server -p vpn-wireguard -p vpn-api-types --all-targets -- -D warnings`
- `git diff --check`

**Manual checks:**
- 用真实飞书审批和目标 Docker 抓包验证审批前、通过后及到期 ACL。

## Suggested Review Order

**审批入口与授权事务**

- 从安全接收、持久化 ACK 到后台处理串起主流程。
  [`feishu_approval_service.rs:209`](../../crates/vpn-server/src/services/feishu_approval_service.rs#L209)

- 身份绑定、建号、延期在单个事务内保持幂等。
  [`access_grant_repo_sqlite.rs:192`](../../crates/vpn-server/src/repositories/access_grant_repo_sqlite.rs#L192)

- 迁移定义账号模式、durable inbox 与逐审批到期事实。
  [`20260731163000_feishu_access_grants.sql:25`](../../migrations/20260731163000_feishu_access_grants.sql#L25)

**路由与服务端强制**

- SQLite 快照合并人工组、有效审批与历史回退。
  [`network_acl_service.rs:48`](../../crates/vpn-server/src/services/network_acl_service.rs#L48)

- 独立 nft table 用精确超时租约实现 fail-closed。
  [`acl.rs:169`](../../crates/vpn-wireguard/src/acl.rs#L169)

- 客户端路由仅合并未到期审批，受管账号不回退。
  [`user_group_repo_sqlite.rs:179`](../../crates/vpn-server/src/repositories/user_group_repo_sqlite.rs#L179)

- 站点路由写入与组路由共锁并双向校验重叠。
  [`peer_service.rs:953`](../../crates/vpn-server/src/services/peer_service.rs#L953)

**生命周期与兼容**

- 启停顺序先保护数据面，并可靠清理遗留 ACL。
  [`main.rs:142`](../../crates/vpn-server/src/main.rs#L142)

- 仅在审批启用时把新飞书账号设为受管模式。
  [`user_repo_sqlite.rs:234`](../../crates/vpn-server/src/repositories/user_repo_sqlite.rs#L234)

- 备份固定一致快照，恢复与 webhook/worker 互斥。
  [`backup.rs:221`](../../crates/vpn-server/src/handlers/backup.rs#L221)

**验证边界**

- 并发实例重放只产生一条 durable inbox 记录。
  [`access_grant_flow.rs:183`](../../crates/vpn-server/tests/access_grant_flow.rs#L183)

- 投递 ID 可变化，其余已验签载荷变化会冲突。
  [`feishu_approval_service.rs:657`](../../crates/vpn-server/src/services/feishu_approval_service.rs#L657)
