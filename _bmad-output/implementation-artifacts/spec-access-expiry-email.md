---
title: 授权到期邮件提醒
type: feature
created: 2026-11-09
status: done
baseline_commit: 5277775
context: []
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

VPN 用户收到审批通过邮件后，授权即将失效时没有主动通知。服务端扫描当前有效的限时用户组授权，默认提前 1 天向用户账号邮箱发送续期提醒，管理员可调整提前天数并编辑提醒邮件模板。

## Boundaries & Constraints

**Always:** 复用管理后台 SMTP 配置、邮件总开关和通知历史。按用户和截止时间合并用户组，展示北京时间截止时间。兼容已有授权和旧备份。SMTP 失败退避重试，授权到期后停止尝试。

**Ask First:** 正式发布和生产环境验证由单独请求触发。

**Never:** 用管理员邮箱或 Webhook 代收提醒，修改用户权限或授权到期时间。

## I/O & Edge-Case Matrix

| 场景 | 输入或状态 | 预期行为 | 异常处理 |
| --- | --- | --- | --- |
| 授权进入窗口 | 同一用户多个用户组在提醒窗口内同一时刻到期 | 合并发送一封，记录通知历史 | SMTP 错误退避 |
| 授权状态变化 | 用户组已有更晚授权或人工组成员关系 | 原截止时间不再提醒该组 | 下次扫描按当前数据判断 |
| 邮件关闭 | 邮件总开关关闭 | 暂停发送 | 开启时扫描仍有效的授权 |
| 历史数据 | 已有授权或恢复旧版备份 | 自动检查仍在提醒窗口内的授权 | 忽略已到期授权及不可用账号 |

</frozen-after-approval>

## Code Map

- `crates/vpn-server/src/repositories/access_grant_repo_sqlite.rs` 保存审批限时授权。
- `crates/vpn-server/src/services/notification_service.rs` 负责邮件配置、发送和记录。
- `crates/vpn-server/src/handlers/backup.rs` 保存及恢复提醒发送状态。
- `frontend/src/pages/NotificationSettingsPage.tsx` 展示事件类型。

## Tasks & Acceptance

- [x] `migrations/`、`access_grant_repo_sqlite.rs` 实现当前有效截止时间筛选、原子认领、租约及持久化退避。
- [x] `notification_service.rs`、`main.rs` 接入邮件 worker。
- [x] `backup.rs` 保存状态，恢复与 worker 互斥。
- [x] `NotificationSettingsPage.tsx`、`docs/notifications.md` 展示提醒类型、配置提前天数及独立邮件模板，并说明触发策略。
- [x] 覆盖时间边界、续期、去重、失败重试、模板和运行时配置及备份兼容。

Given 服务进程重启或多个 worker 尝试认领，when 同一用户截止时间已有发送状态或活跃租约，then 不重复发送。SMTP 已接受但状态未写入的崩溃窗口保留重试语义。

## Spec Change Log

- 并发审查后将当前授权和邮箱查询移至 SMTP 配置加载之后。发送预算受当前授权和认领租约约束，已有成功历史时恢复完成状态。

- 按用户要求将默认窗口改为 1 天，并在通知设置增加提前天数和独立邮件模板。

## Verification

- `cargo test -p vpn-server` 覆盖数据库迁移和服务端回归。

## Suggested Review Order

- 后台任务按邮件开关扫描，发信前核对授权。
  [`notification_service.rs:346`](../../crates/vpn-server/src/services/notification_service.rs#L346)

- 持久化认领、重试和成功状态。
  [`access_grant_repo_sqlite.rs:97`](../../crates/vpn-server/src/repositories/access_grant_repo_sqlite.rs#L97)

- 筛选仍会到期的有效用户组授权。
  [`20261109100000_access_expiry_reminders.sql:1`](../../migrations/20261109100000_access_expiry_reminders.sql#L1)

- 备份与恢复支持提醒状态和旧版本。
  [`backup.rs:235`](../../crates/vpn-server/src/handlers/backup.rs#L235)

- 验证邮件总开关、失败记录及成功后的重试恢复。
  [`notification_service.rs:1148`](../../crates/vpn-server/src/services/notification_service.rs#L1148)
