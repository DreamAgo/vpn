---
title: '完善桌面客户端故障可观测性'
type: 'feature'
created: '2026-07-17T02:10:00+08:00'
status: 'done'
baseline_commit: '18a1c04b46a076147c3f0a986415ad4dd99195d9'
context:
  - '{project-root}/_bmad-output/planning-artifacts/architecture.md'
  - '{project-root}/_bmad-output/planning-artifacts/prd.md'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** Windows Release 客户端连接后若发生 panic、原生崩溃或后台转发任务异常，当前没有持久化日志，窗口消失后用户与运维人员无法定位原因；只有可控的连接错误会短暂显示在 UI。

**Approach:** 为桌面进程增加跨平台滚动文件日志与 panic 捕获，补齐连接生命周期和后台任务的结构化事件，并让 UI 诊断信息明确给出版本、平台和日志位置，使“连接一会后退出”能够留下可交付的排障证据。

## Boundaries & Constraints

**Always:** 日志初始化必须早于 Tauri 与 VPN 管理器启动；日志写入失败不得阻止客户端启动；默认 INFO、支持 `RUST_LOG`；日志保留策略有界；panic、连接、断开、心跳、TUN/路由和后台任务退出均应留下时间与上下文；密码、access/refresh token、私钥、HTTP 认证头及完整认证请求永不记录。

**Ask First:** 引入崩溃转储/上传服务、遥测、远端日志采集，或改变客户端自动重启策略前必须取得用户确认。

**Never:** 不修改 VPN 协议、服务端 API、认证存储格式和路由授权语义；不把秘密写入日志；不因日志系统故障导致 VPN 无法启动；不声称能捕获操作系统在进程外直接终止且未给进程执行机会的情况。

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| 正常运行 | 启动、连接、路由更新、断开、退出 | 按日写入可读日志，包含版本/平台及关键生命周期事件 | 不记录凭证或密钥 |
| Rust panic | 任意线程 panic | panic 消息、位置和线程写入日志并尽量刷盘 | 保持默认 panic 行为，不吞掉崩溃 |
| 日志目录不可写 | 权限或磁盘异常 | 客户端继续运行，回退 stderr/无文件模式 | UI 诊断标明日志不可用及原因 |
| 后台任务异常 | 转发任务 panic 或意外提前结束 | 状态转为 error，日志记录退出原因，UI 可见 | 正常主动断开不得误报 |
| 后端轮询失败 | Tauri command 连续不可用 | UI 明示状态可能过期，不继续把旧 Connected 当作可信 | 恢复后自动回到实时状态 |

</frozen-after-approval>

## Code Map

- `desktop/src-tauri/src/observability.rs` -- 日志目录、滚动/保留、panic hook 与诊断元数据。
- `desktop/src-tauri/src/lib.rs` -- 在 Tauri 初始化前启动可观测性并注册诊断命令。
- `desktop/src-tauri/src/manager.rs` -- 记录连接生命周期并监控转发任务异常结束。
- `crates/vpn-cli/src/daemon.rs` -- 监督数据面与心跳任务，并记录有界心跳健康事件。
- `crates/vpn-cli/src/error.rs` -- 对持久化诊断文本做保守脱敏。
- `crates/vpn-cli/src/wg_userspace.rs` -- 将关键 TUN 写入/转发失败向上层 supervisor 传播。
- `desktop/src-tauri/src/commands.rs` -- 向前端提供脱敏诊断元数据。
- `desktop/src/api.ts` -- 诊断命令的类型与调用封装。
- `desktop/src/App.tsx` -- 复制诊断信息时加入版本、平台和日志路径/错误。
- `desktop/src-tauri/Cargo.toml` -- 增加 tracing 文件输出所需依赖。
- `.github/workflows/ci.yml`、`release.yml` -- 在 GitHub 原生 Windows、macOS、Linux runner 验证桌面端，并确保手动产物构建不推送 Docker。
- `desktop/README.md` -- 记录各平台日志位置、保留策略与排障步骤。

## Tasks & Acceptance

**Execution:**
- [x] `desktop/src-tauri/src/observability.rs`、`Cargo.toml` -- 实现 best-effort 日滚动日志、有限保留、启动元数据和 panic hook，并为纯逻辑路径提供单测。
- [x] `desktop/src-tauri/src/lib.rs`、`commands.rs` -- 最早初始化日志，暴露只读且脱敏的诊断元数据。
- [x] `desktop/src-tauri/src/manager.rs`、`crates/vpn-cli/src/wg_userspace.rs` -- 为连接/断开/心跳添加结构化事件，监控任务 panic/超时/提前返回，把关键转发错误上送 UI，并区分主动停止。
- [x] `desktop/src/api.ts`、`App.tsx` -- 复制诊断信息包含应用版本、OS/架构、日志位置和日志初始化错误；轮询失败时标记状态过期。
- [x] `.github/workflows/ci.yml`、`release.yml` -- PR/主分支在 Windows、macOS、Linux 原生 runner 构建前端并测试、lint 桌面后端；手动 Release 仅生成 artifacts。
- [x] `desktop/README.md` -- 写明 Windows/macOS/Linux 日志位置、如何收集以及不可捕获边界。

**Acceptance Criteria:**
- Given Windows Release 没有控制台，when 客户端启动并连接后异常退出，then 下次排障可在用户数据目录找到含启动与连接阶段事件的日志。
- Given 任意 Rust 线程 panic，when panic hook 执行，then 日志包含线程、位置和消息且不包含凭证、token 或私钥。
- Given 转发任务非主动结束，when manager 观察到任务完成，then UI 状态变为 error 且日志含退出类别；主动断开不产生误报警。
- Given UI 无法连续读取后端状态，when 旧状态为 Connected，then UI 明示状态过期而不是继续展示可信的已连接状态。
- Given 日志目录不可写，when 客户端启动，then GUI 仍可使用且复制诊断信息明确显示日志不可用。

## Spec Change Log

## Design Notes

日志采用低频同步写入与按日文件，优先保证崩溃前末尾事件落盘，启动时清理超出保留期的旧文件。panic hook 先写结构化 ERROR，再调用原 hook，避免改变既有崩溃语义。诊断接口只返回公开运行信息，不读取日志内容或凭证；原生 access violation/WER 转储明确留作后续增强。

## Verification

**Commands:**
- `cd desktop/src-tauri && cargo fmt --check && cargo test && cargo clippy --all-targets -- -D warnings` -- Rust 格式、单测和 lint 全部通过。
- `cd desktop && npm run build` -- TypeScript 与生产前端构建通过。
- `cd desktop/src-tauri && TAURI_CONFIG='{"bundle":{"resources":[]}}' cargo check --target x86_64-pc-windows-gnu` -- 不打包资源，仅验证 Windows 条件编译路径（目标可用时）。

**Manual checks (if no CLI):**
- 启动客户端后确认日志目录生成当日日志；触发测试 panic/后台任务失败时确认日志与 UI 诊断信息符合脱敏和错误态要求。

## Suggested Review Order

**日志与脱敏**

- 从最早入口理解无阻塞滚动日志、保留与诊断设计。
  [`observability.rs:29`](../../desktop/src-tauri/src/observability.rs#L29)

- 限制持久化目标，避免调试级别打开 HTTP 依赖日志。
  [`observability.rs:158`](../../desktop/src-tauri/src/observability.rs#L158)

- 对 panic 文本脱敏，同时保留安全的默认输出行为。
  [`observability.rs:174`](../../desktop/src-tauri/src/observability.rs#L174)

- 任意服务端诊断只保留固定错误码和安全文本。
  [`error.rs:88`](../../crates/vpn-cli/src/error.rs#L88)

**连接生命周期**

- 串行连接并为注册、旧任务清理设置明确边界。
  [`manager.rs:68`](../../desktop/src-tauri/src/manager.rs#L68)

- 超时保留旧 supervisor，阻止带残留路由重连。
  [`manager.rs:206`](../../desktop/src-tauri/src/manager.rs#L206)

- 统一监督数据面与心跳，区分主动停止和内部故障。
  [`daemon.rs:417`](../../crates/vpn-cli/src/daemon.rs#L417)

- 聚合网络故障并确保所有退出路径清理路由。
  [`wg_userspace.rs:226`](../../crates/vpn-cli/src/wg_userspace.rs#L226)

- 补齐排空队列中的 TUN 写失败传播。
  [`wg_userspace.rs:451`](../../crates/vpn-cli/src/wg_userspace.rs#L451)

**界面诊断**

- 连续轮询失败才标记过期，并拒绝旧请求覆盖恢复结果。
  [`App.tsx:90`](../../desktop/src/App.tsx#L90)

- 复制时重新读取版本、平台、日志路径和初始化错误。
  [`App.tsx:597`](../../desktop/src/App.tsx#L597)

- 后端只暴露脱敏、只读的诊断元数据。
  [`commands.rs:103`](../../desktop/src-tauri/src/commands.rs#L103)

**跨平台运维**

- 三种原生 runner 对桌面前后端执行严格构建检查。
  [`ci.yml:97`](../../.github/workflows/ci.yml#L97)

- 手动产物构建不再意外推送 Docker 镜像。
  [`release.yml:544`](../../.github/workflows/release.yml#L544)

- 三平台日志位置、收集步骤和原生崩溃边界说明。
  [`README.md:141`](../../desktop/README.md#L141)
