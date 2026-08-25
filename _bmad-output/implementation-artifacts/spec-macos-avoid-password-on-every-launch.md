---
title: 'macOS 启动时不再重复要求管理员密码'
type: 'bugfix'
created: '2026-08-25'
status: 'done'
baseline_commit: 'dfc8eb07cea7ba1f466e771c04425a7bf9270ce1'
context: []
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** macOS release 版每次启动都用 `osascript` 将整个 GUI 以 root 身份重启，因此每次弹管理员密码；WebView、自动启动和用户数据也长期处于不必要的高权限环境。

**Approach:** 改为普通用户 GUI + 常驻 root 隧道 helper。helper 首次安装及二进制升级时各授权一次，日常启动通过受限 Unix socket 控制，不再弹密码。

## Boundaries & Constraints

**Always:** 仅 helper 负责 TUN、路由和转发；helper 缺失/版本变化时才请求授权；IPC 限制权限、长度并验证当前控制台用户；日志不得泄露令牌、私钥或 PSK；升级后最多要求一次账户重新登录。

**Ask First:** 引入 Apple Developer 证书、Network Extension、SMAppService/SMJobBless，或改变 Windows/Linux 架构前先确认。

**Never:** 不缓存管理员密码，不修改 sudoers，不开放全局可写 socket，不以 root 运行 Tauri/WebView，不关闭 macOS 安全机制。

## I/O & Edge-Case Matrix

| Scenario | State | Expected Behavior | Error Handling |
|---|---|---|---|
| 首次使用 | helper 未安装 | 授权一次并安装、加载 | 取消后 GUI 可用，连接页提示安装 |
| 日常启动 | helper 版本匹配 | 无密码框，可正常连接 | 未响应时尝试启动并报告原因 |
| 客户端升级 | helper 版本不同 | 本次授权并原子替换 | 失败时保留旧版本 |
| 非法 IPC | 非控制台用户/超限请求 | 拒绝且不执行网络操作 | 脱敏记录并关闭连接 |
| 会话恢复 | 用户凭据存在 | 保持登录状态 | 旧 root 凭据不可迁移则提示一次登录 |

</frozen-after-approval>

## Code Map

- `desktop/src-tauri/src/lib.rs` -- 移除每次启动的整进程提权。
- `desktop/src-tauri/src/commands.rs`、`desktop/src-tauri/src/manager.rs` -- macOS 隧道操作改走 helper，其他平台不变。
- `desktop/src-tauri/src/main.rs`、`desktop/src-tauri/src/macos_helper.rs` -- GUI/helper 双模式、安装和 launchd 生命周期。
- `crates/vpn-cli/src/ipc.rs` -- helper IPC、长度限制和 Unix peer 校验。
- `.github/workflows/release.yml` -- 验证最终 App 包内的 helper 入口；helper 复用同一签名可执行文件。
- `desktop/README.md` -- 权限、升级、卸载和排障说明。

## Tasks & Acceptance

**Execution:**
- [x] `crates/vpn-cli/src/ipc.rs` -- 添加最小化 helper 协议、限长编解码与日志快照响应。
- [x] `desktop/src-tauri/src/macos_helper.rs` -- 实现限并发/限时且校验当前 console UID 的 IPC；用户切换时断开隧道并原子移交 socket。
- [x] `desktop/src-tauri/src/macos_helper.rs` -- 安装前 single-flight；特权侧从固定数据生成 plist、复制后校验 GUI 预计算的 SHA-256，并以覆盖 bootout 后所有失败点的 trap 事务升级。
- [x] `desktop/src-tauri/src/macos_helper.rs` -- 以已安装文件摘要区分缺失、版本变化和暂时不可达；匹配但不可达时只尝试无授权恢复并报告，禁止重装。
- [x] `desktop/src-tauri/src/lib.rs`、`desktop/src-tauri/src/commands.rs`、`desktop/src-tauri/src/manager.rs`、`desktop/src-tauri/src/main.rs` -- 普通用户 GUI 代理 macOS 隧道操作；helper 致命失败返回非零；断开失败时不得清除登录凭据。
- [x] `desktop/src-tauri/src/macos_helper.rs`、`desktop/src-tauri/src/observability.rs` -- helper 日志 root 私有、限量轮转，只经认证 IPC 返回限长且脱敏的快照。
- [x] `.github/workflows/release.yml` -- 验证 App 包内 helper 入口、非 root 失败码和构建身份。
- [x] `desktop/README.md`、`docs/client.md` -- 更新使用、恢复与卸载说明。

**Acceptance Criteria:**
- Given helper 已安装且版本匹配，when 连续重启 GUI 三次，then 不弹管理员密码且连接、断开、状态均正常。
- Given helper 未安装，when 首次连接，then 仅安装时授权一次，成功后无需重启 GUI。
- Given Windows/Linux 构建，when 运行现有测试，then 原连接路径不变。
- Given 其他本机用户连接 socket，when 发送控制请求，then helper 拒绝且隧道状态不变。

## Spec Change Log

- 2026-08-25（审查回环 1）：Blind/Edge/Acceptance 审查发现用户可写临时 plist 与可替换 App 路径存在 root 安装 TOCTOU，且不可达 helper 会误触发重装；补充特权侧固定 plist、复制后摘要校验、完整事务回滚、安装 single-flight、当前 console UID、不可达分类、私有轮转日志及注销断开约束。避免任意 LaunchDaemon/root 二进制注入、重复授权、用户切换越权、半升级和“已注销但隧道仍在”。KEEP：普通用户 GUI + 最小 root helper、owner-only socket、限长协议、复用同一 App 可执行文件、其他平台原路径、日志面板与脱敏测试。
- 2026-08-25（审查回环 2）：第二轮审查发现摘要初始化失败会延迟重建信任基线、旧连接可跨 console generation 执行、断开失败仍会移交 socket、不同用户安装锁不能保护系统级事务、旧 helper 不可达时无法升级。补充 fail-closed 摘要状态、helper 启动时固定自身摘要、会话 generation 与切换互斥、断开成功后才移交、root 全局事务锁、不可达旧 helper 由授权事务 bootout 后替换，以及 CI 将程序自报摘要与 `shasum` 比较。避免旧用户越权、残留隧道被新用户接管、并发升级损坏和永久不可升级。KEEP：回环 1 的事务回滚、私有日志、安装后摘要匹配、匹配但不可达不重装，以及普通用户 GUI 架构。
- 2026-08-25（审查回环 3）：第三轮审查发现请求持有会话锁会延迟用户切换、PID 目录锁存在永久 stale 窗口，且注销的状态查询/断开/删凭据之间允许旧 token 并发重连。补充 watch generation 取消与响应前复核、所有断开有界、内核 `lockf` root 全局锁、用户侧安装锁超时，以及原子 `PrepareLogout`：先封禁当前 refresh-token 摘要、串行等待连接结束并断开，成功后才允许 GUI 删除凭据。避免旧用户请求跨会话完成、安装永久卡死和“注销后隧道又被旧凭据拉起”。KEEP：回环 2 的 fail-closed 摘要、helper 启动身份固定、断开成功才移交 socket、不可达升级与构建身份 CI。
- 2026-08-25（审查回环 4）：第四轮审查发现 generation 取消可能落在数据面已启动但 manager 尚未登记句柄的窗口，且仅内存保存的注销 token 摘要会在 helper 重启/升级后失效；同时修正变更请求的闭合超时预算和日志写满后永久静默。补充连接取消清理 guard（任意 await 点 drop 都发送 shutdown）、root-only 持久 token 摘要栅栏、135 秒客户端事务预算、console 复检失败立即 root-only 并断开，以及 4×2 MiB 固定容量日志轮转。避免无人管理的 root TUN/路由、helper 重启后旧凭据复活、后台迟到提交和审计日志永久停写。KEEP：回环 3 的原子 PrepareLogout、watch generation、`lockf` 与完整安装回滚。
- 2026-08-25（用户批准继续后的架构修正）：终审确认仅发送 shutdown 不能提供“清理完成”屏障。将完整建连流程放入 manager-owned task，并用 operation epoch 使 disconnect 在等待锁前取消建连；建连若已创建数据面则等待 forward 清理完成，清理超时时把句柄登记为 supervisor，使 disconnect 继续失败并保持 root-only，完成前绝不移交 socket。console 切换增加 `/dev/console` kqueue 事件，轮询仅兜底；注销事务预算增至 210 秒并限制 guard 等待；持久 token fence 采用最多 256 项、满后 fail-closed、32 KiB 读取上限、文件及目录同步的原子重写；安装开始前恢复遗留备份，并在 bootout/替换边界执行磁盘同步。避免 detached 数据面、迟到建连、跨升级旧凭据复活、状态文件膨胀和断电后丢失最后可启动 helper。KEEP：所有既有 IPC 身份校验、fail-closed 构建摘要、root `lockf`、固定 plist、日志有界轮转和事务回滚。

## Design Notes

macOS 不能安全记住 sudo 密码；由 launchd 持久管理最小 root helper 才能消除重复授权。账户密码不发送给 helper；连接令牌只经已验证的本地连接临时传递。

实现复用 App bundle 内的同一可执行文件，通过 `--privileged-helper` 在 Tauri 初始化前切换为无 GUI 模式。GUI 最早期固定源二进制 SHA-256，失败后本次进程永久禁止安装；helper 启动时固定运行映像摘要。root 全局锁保护完整替换事务，仅安装复制后摘要一致的文件，plist 由固定数据生成，不读取用户可写临时 plist。IPC 请求在执行前按 console generation 再校验，用户切换先将 socket 收回 root 并断开，成功后才移交。helper 日志位于 root-only 轮转目录，GUI 只能通过已认证 IPC 获取脱敏快照。

## Verification

**Commands:**
- `cargo test -p vpn-cli ipc`
- `cd desktop/src-tauri && cargo test && cargo clippy --all-targets -- -D warnings`
- `cd desktop && npm run build`
- macOS 真机首次授权后连续启动三次并连接/断开；预期后两次无授权框。

## Suggested Review Order

**权限边界与进程入口**

- 同一二进制在 Tauri 启动前切换为受限 root helper。
  [`lib.rs:123`](../../desktop/src-tauri/src/lib.rs#L123)

- helper 固定自身身份并启动认证、有界 IPC 服务。
  [`macos_helper.rs:79`](../../desktop/src-tauri/src/macos_helper.rs#L79)

- 最小协议只暴露连接、断开、状态、注销和日志。
  [`ipc.rs:78`](../../crates/vpn-cli/src/ipc.rs#L78)

**建连、取消与清理安全**

- manager-owned 建连任务用 epoch 封堵迟到 TUN 提交。
  [`manager.rs:98`](../../desktop/src-tauri/src/manager.rs#L98)

- 断开等待真实清理，超时或失败时永久 fail-closed。
  [`manager.rs:379`](../../desktop/src-tauri/src/manager.rs#L379)

- 运行故障与路由清理失败分离，仅后者阻止重连。
  [`daemon.rs:533`](../../crates/vpn-cli/src/daemon.rs#L533)

**安装、升级与注销持久性**

- root `lockf` 事务固定 plist，校验摘要并完整回滚。
  [`macos_helper.rs:990`](../../desktop/src-tauri/src/macos_helper.rs#L990)

- 注销 token fence 原子重写且允许目录 fsync 失败后重试。
  [`macos_helper.rs:828`](../../desktop/src-tauri/src/macos_helper.rs#L828)

**交付与运维**

- CI 核对 bundle 内实际 SHA-256、自报身份和非 root 拒绝。
  [`release.yml:338`](../../.github/workflows/release.yml#L338)

- 文档说明首次授权、升级、日志和 helper 卸载路径。
  [`README.md:35`](../../desktop/README.md#L35)
