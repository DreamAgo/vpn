# 架构概览

本文是面向贡献者的架构速览。完整设计见 [`_bmad-output/planning-artifacts/architecture.md`](../_bmad-output/planning-artifacts/architecture.md)。

## Workspace 与依赖方向

```
vpn-api-types  ← 纯 DTO（serde，无 IO）
vpn-core       ← 领域错误(AppError) + trait（PasswordHasher / TokenIssuer / Clock）
vpn-wireguard  ← 密钥 / IP 池 / WireGuardControl 抽象（依赖 vpn-core）
vpn-platform   ← 客户端跨平台：TunDevice / CredentialStore / DaemonRuntime（自有错误）
vpn-server     ← HTTP + 内嵌前端 + 数据平面装配（依赖 api-types/core/wireguard）
vpn-cli        ← 客户端 CLI + daemon（依赖 api-types/core/platform/wireguard）
```

依赖单向向上，`vpn-api-types` 不依赖任何 IO crate，保证前后端契约纯净。

## 控制平面（vpn-server）

- **HTTP**：axum 0.8。中间件链（ServiceBuilder）：RequestId → Trace → 业务；认证路由额外经 `require_auth`，写操作再经 `audit_layer`。
- **认证**：argon2id 密码哈希；Access Token = JWT RS256（15 分钟），Refresh Token = 不透明随机串（sha256 存库、可撤销）。`CurrentUser` / `RequireAdmin` 提取器做鉴权。
- **持久化**：SQLite + sqlx，迁移在 `migrations/`。仓储层用元组 + `From` 映射避免复杂类型。
- **状态**：`AppState` 持有各 service（auth / user / peer / audit），启动时在 `main.rs` 装配并注入。
- **后台任务**：节点离线扫描（30s）、审计日志清理（24h）。

## 数据平面（vpn-wireguard + vpn-server）

- `WireGuardControl` trait 抽象「在 WireGuard 上增删 peer」。生产实现基于 boringtun 用户态（系统层，需真机）；测试与无 root 环境用 `NoopWireGuardControl`。
- `IpPool` 从子网静态分配 IP（跳过网络/服务端/广播），同一用户重连复用同一 IP；启动时由 `peers` 表回填。
- 服务端 WireGuard 密钥对持久化于 `system_config` KV 表，启动时 load-or-generate。

## 客户端（vpn-cli + vpn-platform）

- `vpn-platform` 把对 OS 的依赖收敛到三个 trait：`TunDevice`（tun-rs）、`CredentialStore`（keyring + 加密文件降级）、`DaemonRuntime`（systemd/launchd/Windows Service）。
- `vpn-cli`：clap 命令 + reqwest API client（信封解包 + 401 自动刷新）+ Unix Socket/Named Pipe IPC（CLI↔daemon）+ daemon 状态机 + 指数退避重连 + 网络变化检测。

## 前端（frontend）

- React 19 + Vite + AntD Pro，构建产物由 `rust-embed` 内嵌进 `vpn-server` 二进制，单文件交付，SPA fallback 路由。
- axios 拦截器自动 snake_case↔camelCase 并解包 `ApiResponse` 信封；401 静默刷新重放。

## 测试策略

- 各 crate 单元测试（纯逻辑：IP 池、退避算法、密码校验、信封解包、服务文件渲染等）。
- 服务端 HTTP 集成测试（`crates/vpn-server/tests/*`）：用内存 SQLite + `NoopWireGuardControl` 端到端跑通认证、用户 CRUD、节点注册、审计与强制下线流程。
- 需真机/特权的路径以 `#[ignore]` 标注。
