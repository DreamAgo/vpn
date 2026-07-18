---
title: '客户端详细连接日志与日志弹窗'
type: 'feature'
created: '2026-07-18T00:00:00+08:00'
status: 'done'
baseline_commit: '8ec70495fe99365bb8259ad818fafdfbd985560a'
context:
  - '{project-root}/_bmad-output/implementation-artifacts/spec-desktop-client-operability.md'
  - '{project-root}/_bmad-output/planning-artifacts/architecture.md'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** 客户端已有滚动日志，但连接过程只有少数结果事件，难以像 OpenVPN 一样定位凭证、注册、TUN、UDP、路由、心跳或清理故障；GUI 也不能直接查看日志。

**Approach:** 补充带阶段、结果、耗时和关联编号的结构化日志；通过受控 Tauri 命令读取近期内容，从设置打开独立日志弹窗，提供自动/手动刷新、复制和自动滚动。

## Boundaries & Constraints

**Always:** 默认 INFO 可还原关键步骤；失败只记安全摘要；读取来源固定、时间正序、最大 500 行/256 KiB，返回前二次脱敏；关闭弹窗即停止刷新；读取失败不影响连接；保留 7 天/7 文件策略。

**Ask First:** 远端上传、遥测、崩溃转储、日志删除、任意路径读取或改变连接语义。

**Never:** 不记录密码、token、私钥、Authorization、完整请求/响应；不开放第三方 HTTP trace；不接受前端路径；不把 TUN/路由完成描述成已握手。

## I/O & Edge-Case Matrix

| 场景 | 输入 / 状态 | 预期行为 | 错误处理 |
|---|---|---|---|
| 正常连接 | 点击连接 | 顺序显示凭证、注册、旧任务清理、TUN、UDP、路由、任务和心跳 | 不输出秘密 |
| 阶段失败 | DNS/API/TUN/路由失败 | 记录阶段、耗时和脱敏摘要 | 弹窗保留最后成功快照并提示错误 |
| 持续写入 | 弹窗打开 | 每 2 秒刷新；位于底部时跟随 | 关闭即停止轮询 |
| 大文件/轮转 | 超过上限 | 返回近期内容并标记截断 | 忽略无关项和符号链接 |
| 日志不可用 | 不存在、无权限、非 UTF-8 | 展示空态或错误 | 有损解码且读取有界 |

</frozen-after-approval>

## Code Map

- `desktop/src-tauri/src/observability.rs` -- 安全 tail、筛选、上限、脱敏与测试。
- `desktop/src-tauri/src/manager.rs`、`crates/vpn-cli/src/{daemon.rs,wg_userspace.rs}` -- 分阶段连接日志。
- `desktop/src-tauri/src/{commands.rs,lib.rs}` -- 只读近期日志命令。
- `desktop/src/api.ts` -- 日志快照 DTO、调用和预览数据。
- `desktop/src/{App.tsx,styles.css}` -- 日志入口、弹窗和交互。
- `desktop/README.md` -- 使用方式、截断与安全边界。

## Tasks & Acceptance

**Execution:**
- [x] `observability.rs`、`commands.rs`、`lib.rs` -- 实现固定来源、有界、二次脱敏的快照，测试轮转、上限、无关项、符号链接和非 UTF-8。
- [x] `manager.rs`、`daemon.rs`、`wg_userspace.rs` -- 为凭证、注册、清理、endpoint、TUN、UDP、路由、心跳和退出记录阶段、结果与耗时。
- [x] `api.ts`、`App.tsx`、`styles.css` -- 增加弹窗；实现仅可见时轮询、刷新、复制、滚动暂停、截断/空态/错误态。
- [x] `desktop/README.md` -- 记录日志弹窗和排障方法。

**Acceptance Criteria:**
- Given 默认 INFO，when 连接结束，then 可顺序定位控制面或数据面阶段及耗时。
- Given 弹窗打开且日志增长，when 用户位于底部或向上阅读，then 更新不会打断阅读。
- Given 日志含秘密、超长内容或轮转，when 请求快照，then 内容脱敏、有界、正序且标记截断。
- Given 日志不可读，when 打开弹窗，then 可重试且连接任务不受影响。

## Spec Change Log

## Design Notes

后端选择文件、限制读取并脱敏；前端只轮询文本快照。现有“连接日志”改名“近期事件”。弹窗不创建第二个原生窗口。

## Verification

**Commands:**
- `cd desktop/src-tauri && cargo fmt --check && cargo test && cargo clippy --all-targets -- -D warnings`
- `cd desktop && npm run build`
- `cargo test -p vpn-cli`

**Manual checks:**
- 在 360×560 窗口验证弹窗开关、刷新、复制、滚动暂停、空态、错误态、长行和截断；真机连接后检查阶段顺序及脱敏。

## Suggested Review Order

**日志弹窗体验**

- 从独立弹窗入口理解刷新、冻结阅读和错误保留策略。
  [`App.tsx:815`](../../desktop/src/App.tsx#L815)

- 前端调用设置五秒超时，避免后端读取悬挂界面。
  [`api.ts:95`](../../desktop/src/api.ts#L95)

**安全日志边界**

- 固定目录、有界尾读、轮转合并和二次脱敏集中在此。
  [`observability.rs:127`](../../desktop/src-tauri/src/observability.rs#L127)

- 原子拒绝链接文件，避免目录检查后的替换竞态。
  [`observability.rs:293`](../../desktop/src-tauri/src/observability.rs#L293)

- 阻塞文件读取移出 Tauri 命令执行线程。
  [`commands.rs:119`](../../desktop/src-tauri/src/commands.rs#L119)

**连接阶段追踪**

- 尝试编号串联凭证、注册、数据面和最终失败。
  [`manager.rs:73`](../../desktop/src-tauri/src/manager.rs#L73)

- 控制面刷新、注册与心跳记录阶段结果及耗时。
  [`daemon.rs:208`](../../crates/vpn-cli/src/daemon.rs#L208)

- 数据面细分 endpoint、TUN、UDP、路由和清理状态。
  [`wg_userspace.rs:253`](../../crates/vpn-cli/src/wg_userspace.rs#L253)

**支持与验证**

- 应用文档说明入口、保留策略、上限和排障顺序。
  [`README.md:141`](../../desktop/README.md#L141)

- 快照测试覆盖顺序、边界、脱敏、链接和非法编码。
  [`observability.rs:490`](../../desktop/src-tauri/src/observability.rs#L490)
