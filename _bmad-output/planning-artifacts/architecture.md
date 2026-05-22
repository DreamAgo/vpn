---
stepsCompleted: [1, 2, 3, 4, 5, 6, 7, 8]
lastStep: 8
status: 'complete'
completedAt: '2026-05-22'
inputDocuments:
  - prd.md
  - ux-design-specification.md
  - research/technical-rust-vpn-openvpn-research-2026-05-11.md
  - research/technical-vpn-protocol-architecture-research-2026-05-11.md
  - research/technical-rust-vpn-security-integration-research-2026-05-11.md
  - research/technical-rust-vpn-architecture-patterns-research-2026-05-11.md
  - research/technical-rust-vpn-implementation-strategy-research-2026-05-11.md
  - research/technical-rust-vpn-risk-assessment-research-2026-05-11.md
  - research/technical-rust-vpn-control-plane-api-research-2026-05-11.md
workflowType: 'architecture'
project_name: 'vpn'
user_name: 'Shangguanjunjie'
date: '2026-05-11'
---

# Architecture Decision Document

_This document builds collaboratively through step-by-step discovery. Sections are appended as we work through each architectural decision together._

## Project Context Analysis

### Requirements Overview

**Functional Requirements:**

PRD 定义 **63 项功能需求**（MVP 49 / Growth 8 / Vision 6），分 8 大能力域。架构必须支持以下核心能力域：

| 能力域 | FR 数（MVP） | 架构含义 |
|-------|------------|---------|
| A. 部署与初始化 | 5 | 自启动 + ACME 自动 HTTPS + 引导式 setup + 重启恢复 → 启动子系统、证书管理、配置持久化 |
| B. 账号与认证 | 7 | argon2id + JWT RS256 + Refresh Token + 限速锁定 → 完整认证子系统 |
| C. 用户管理（admin） | 7 | 用户 CRUD + 级联清理（用户→peer→token）→ 事务一致性要求 |
| D. 节点（Peer）管理 | 9 | WireGuard 公钥注册 + 心跳 + IP 分配 + 强制下线 → WireGuard UAPI 抽象层 |
| E. 客户端体验 | 9 | CLI + daemon + 跨平台凭证存储 + 跨平台安装包 → 客户端架构与 IPC |
| F. 隧道运维 | 5 | boringtun 集成 + TUN 抽象 + 断线重连 + 网络变化检测 → 数据平面核心 |
| G. 审计与可见性 | 5 | 审计日志（保留 6 月）+ WebSocket 实时推送 → 事件系统与持久化 |
| H. 安全防护 | 6 | 限速、HTTPS 强制、密钥隔离、原子清理 → 安全横切关注点 |

**Non-Functional Requirements:**

PRD 定义 **43 项非功能需求**（MVP 38 / Growth 5），架构必须满足：

| 类别 | 关键约束 | 架构影响 |
|------|---------|---------|
| **Performance** | 单隧道 ≥ 100Mbps；50 节点 CPU < 30%；API p95 ≤ 300ms | 数据平面用 boringtun userspace + tokio 异步；控制平面单进程足够 |
| **Security** | argon2id + JWT RS256 + WireGuard ChaCha20-Poly1305 + TLS 1.2+ + 非 root + CAP_NET_ADMIN | 强制安全架构层；密码学库选型固定 |
| **Reliability** | 30 天连续运行；重连成功率 ≥ 99%；30s 内重启恢复；WireGuard timer ≤ 250ms | 必须持久化所有 peer 配置；独立 timer 任务；优雅关闭 |
| **Scalability** | 50 并发活跃节点（MVP）；500 注册账号；SQLite → PostgreSQL 平滑迁移 | Repository 模式抽象数据库；不预优化 |
| **Compatibility** | Linux 5.6+ 服务端；macOS 11+/Windows 10+/Linux 客户端三平台对等 | 客户端需跨平台抽象层（TUN、凭证存储、daemon） |
| **Operability** | 单容器部署 5 分钟；零 SSH 日常运维；启动失败明确错误 | 单二进制部署；嵌入静态前端；明确启动校验 |
| **Maintainability** | 70% 单元测试覆盖；3 个 E2E 测试；CI 三平台矩阵 | 模块化设计；接口抽象；测试可注入 |

**Scale & Complexity:**

- **项目复杂度：高（high）**
- **主要技术领域：全栈（Rust 后端 + Rust CLI 客户端 + React 前端 + 系统编程）**
- **预估架构组件数：12–15 个核心模块**
- 复杂度来源：
  - VPN 协议实现（boringtun userspace 集成 + TUN 设备抽象）
  - 跨平台系统编程（Linux netlink / macOS utun / Windows WinTUN）
  - 控制平面与数据平面在单进程内协作（共享状态、事件同步）
  - 实时性要求（WireGuard timer 100ms 精度、心跳 30s）
  - 双端架构（服务端单体 + 客户端 daemon + CLI + 前端 SPA）

### Technical Constraints & Dependencies

**已通过研究固定的硬性约束：**

| 决策 | 选择 | 不可妥协原因 |
|------|------|------------|
| 数据平面协议 | WireGuard | 性能 2x OpenVPN；Rust 生态唯一生产级实现 |
| WireGuard 实现 | `boringtun` | Cloudflare 生产验证；纯 Rust 无 C 依赖 |
| TUN 库 | `tun-rs` v2 | 唯一活跃维护的跨平台异步 TUN |
| WireGuard 接口管理 | `defguard_wireguard_rs` | 唯一跨平台统一 API（Linux/macOS/Windows） |
| 异步运行时 | `tokio` | 所有上游库依赖 |
| Web 框架 | `axum` 0.7 | 与 tokio 同源，运行时统一；ProComponents 整合简单 |
| 密码哈希 | `argon2` (argon2id) | OWASP 2024 推荐；PRD 强制 |
| JWT | `jsonwebtoken` (RS256) | PRD 强制非对称签名 |
| 数据库 | `sqlx` + SQLite (MVP)/PostgreSQL (后续) | 编译时 SQL 校验；零外部依赖（MVP）|
| 前端 | React 18 + AntD Pro 5 | UX Spec §5 决策；ProTable 节省 80% 代码 |

**关键技术陷阱（必须在架构中规避）：**

1. **boringtun timer 不调用 → 静默断线**：必须设计独立 tokio task 每 ≤ 250ms 调用 `update_timers()`，并暴露监控指标
2. **`std::sync::Mutex` 跨 `.await` → 死锁**：架构层规定全局使用 `tokio::sync::*`；CI 集成 clippy 检查
3. **`Tunn` 非 Send**：必须用 `Arc<Mutex<Tunn>>` 或 channel actor 模式串行化访问
4. **跨平台 TUN 行为差异**：必须用 `defguard_wireguard_rs` 抽象层隔离；CI 三平台矩阵

**外部依赖：**

- ACME 证书提供方（Let's Encrypt）→ 网络可达性依赖
- 操作系统：Linux 内核 ≥ 4.19（推荐 5.6）；macOS ≥ 11；Windows 10 (1809+)
- WinTun.dll（Windows 客户端嵌入）

### Cross-Cutting Concerns Identified

跨多个模块的关注点（架构必须统一处理）：

| 关注点 | 影响模块 | 架构应对 |
|-------|---------|---------|
| **跨平台抽象** | 客户端、TUN、凭证存储 | Trait 抽象（`TunDevice` / `CredentialStore` / `DaemonRuntime`）+ 平台特定实现 |
| **认证与授权** | 所有 API 端点、WebSocket、CLI | Tower middleware 统一拦截；RBAC（admin/user）通过中间件实现 |
| **审计日志** | 所有写操作（用户、节点、配置） | 中间件自动记录；6 个月保留通过 cron 清理 |
| **错误处理** | 全应用 | `thiserror` 定义业务错误；`IntoResponse` 统一 API 错误响应 |
| **可观测性** | 全应用 | `tracing` + 结构化日志；MVP 简化（不引入 OpenTelemetry，留 Growth） |
| **配置管理** | 服务端 + 客户端 | 环境变量 + 配置文件 + 数据库三层；服务端配置可热加载 |
| **优雅关闭** | 服务端整体 | SIGTERM → 停止接收新请求 → 等待 in-flight → 关闭 WireGuard → 退出 |
| **重启恢复** | WireGuard peer 配置、活跃 Token | 启动时从 DB 重建 WireGuard；Token 验证靠签名（无需持久化白名单） |
| **限速防护** | 登录、心跳、API | `tower-governor`（GCRA）+ Redis 计数（Refresh Token 黑名单） |
| **服务端/客户端通信** | API、WebSocket、心跳 | 统一 JSON Schema；客户端 daemon ↔ CLI 用 Unix Socket / Named Pipe |

### Architectural Scope Boundaries（MVP 范围明确）

**MVP 架构包含：**

- 单进程服务端（Axum + WireGuard 数据平面 + 嵌入式静态前端）
- 客户端二进制（CLI + daemon + TUN 管理 + 凭证存储）
- React SPA（独立构建，由服务端静态服务）
- SQLite 单文件存储（含 migrations）
- Docker 单容器部署（含静态前端）

**MVP 架构明确排除：**

- 微服务拆分（控制/数据/信令分离）
- 高可用集群（多服务端 + Raft 共识）
- 消息队列（Kafka/RabbitMQ）
- PostgreSQL（迁移路径预留但不实现）
- Mesh P2P 拓扑与 NAT 穿透
- Tauri 桌面 GUI 客户端
- 移动端
- LDAP/SSO 集成
- Prometheus/Grafana 集成（指标接口预留）

## Starter Template Evaluation

### Primary Technology Domain

本项目是 **多模块系统级 Rust 项目 + React 管理后台**，分两个独立技术栈：

1. **后端 + 客户端**：Rust workspace 多 binary（vpn-server / vpn-cli / vpn-daemon）+ 多 lib crate
2. **前端**：React 18 SPA（独立构建 + 由服务端静态服务）

不存在覆盖完整系统的"全栈 starter"。需分别评估。

### Starter Options Considered

#### Rust 部分

| 选项 | 评估 |
|------|------|
| `cargo new --workspace`（手动） | **推荐**。Rust workspace boilerplate 极少，手动可控、不引入隐性依赖 |
| `launchbadge/realworld-axum-sqlx` | 单 crate 参考实现（sqlx 官方），不适合 workspace；仅作架构参考 |
| `Judduuk/rust-starter-pack` | workspace + axum + sqlx + Docker，作为目录结构参考而非直接 fork |
| `Axium`（含 JWT/RBAC/Redis/S3） | 功能过多，强加架构选择；不适合 |
| `cargo shuttle init --template axum` | 绑定 Shuttle 平台，不符自托管定位 |
| `cargo-generate` 模板 | 社区模板更新慢（axum 0.7→0.8 多次破坏性升级）；不推荐 |

**结论：手动 `cargo new --workspace`**，参考 `realworld-axum-sqlx` 抄目录结构与最佳实践，参考 `Judduuk/rust-starter-pack` 抄 workspace 组织。

#### 前端部分

| 选项 | 评估 |
|------|------|
| AntD Pro V6 全家桶（Umi Max） | 重量级（约定式路由 + dva + 微前端 + 插件体系）；对 8 页项目过度工程 |
| **Vite + React + AntD 5 + ProComponents 手动** | **推荐**。轻量、构建快、生态通用、不被 Umi 绑死 |
| `pro-cli` (`pro create`) | 不再官方主推；V6 已改为 `git clone` 模式 |
| `condorheroblog/react-antd-admin` | 完整模板（含路由权限），作为目录结构参考 |
| 自建从零搭建 | 不必要，Vite 官方 React-TS 模板 + 手动加 antd 即可 |

**结论：Vite + 手动组装**。Vite 官方 `npm create vite@latest -- --template react-ts` 起步，手动添加 antd / pro-components / 路由 / 状态管理。

### Selected Starter: 手动初始化（Rust workspace + Vite React-TS）

**Rationale for Selection：**

不采用任何全套 starter 模板的原因：

1. **现有 starter 都不匹配项目独特性** — VPN 项目需要 `boringtun` + `tun-rs` + `defguard_wireguard_rs` 系统级依赖，没有 starter 包含这些
2. **手动初始化的 boilerplate 极少** — Rust workspace 只需根 `Cargo.toml` 配 `[workspace]` + `[workspace.dependencies]`；Vite 项目 `npm create` 即可
3. **避免隐性约定锁定** — Starter 引入的目录结构、命名约定一旦不合适，移除成本高
4. **PRD 与 UX Spec 已明确所有技术决策** — 没有"需要 starter 来引导我决策"的需求

**Initialization Commands：**

```bash
# 1. 创建 Rust workspace（项目根目录）
mkdir vpn && cd vpn
cargo new --vcs git .  # 主 Cargo.toml 作为 workspace 根
# 编辑 Cargo.toml 加 [workspace] 配置（详见后续模块划分章节）

# 2. 创建子 crate
cargo new --bin crates/vpn-server
cargo new --bin crates/vpn-cli
cargo new --lib crates/vpn-core
cargo new --lib crates/vpn-wireguard
cargo new --lib crates/vpn-platform
cargo new --lib crates/vpn-api-types

# 3. 初始化前端（在仓库根目录创建 frontend/）
npm create vite@latest frontend -- --template react-ts
cd frontend
npm install antd @ant-design/pro-components @ant-design/icons
npm install @tanstack/react-query zustand axios react-router-dom dayjs

# 4. （后续）项目根添加 Justfile / Makefile 统一构建命令
```

**Architectural Decisions Provided by Manual Setup（即由本架构文档定义）：**

**Language & Runtime：**

- 后端：Rust stable（≥ 1.82）+ tokio multi-thread runtime
- 前端：TypeScript 5.x strict + React 18.x
- 构建：Cargo（后端）+ Vite 5（前端）

**Styling Solution：**

- AntD 5 内置组件样式
- AntD CSS-in-JS（`@ant-design/cssinjs`，自动开启）
- 全局微调通过 ConfigProvider + theme token（不引入 Tailwind/Sass）

**Build Tooling：**

- 后端：`cargo build --release` + `cargo-chef` Docker 缓存
- 前端：Vite 生产构建 → 静态文件 → Rust 服务端嵌入（`rust-embed` 或独立 nginx）
- 跨平台编译：`cross` 工具

**Testing Framework：**

- 后端：Cargo 内置 `cargo test` + `tokio::test` + `sqlx::test`（隔离测试数据库）
- 前端：Vitest + React Testing Library + jest-axe（无障碍）
- 端到端：Docker Compose 启动 server + 多 client 容器

**Code Organization：**

```
vpn/                            # 项目根
├── Cargo.toml                  # workspace 根（含 [workspace.dependencies]）
├── Cargo.lock
├── crates/                     # Rust 子 crate
│   ├── vpn-server/            # 服务端二进制
│   ├── vpn-cli/               # 客户端二进制（CLI + daemon）
│   ├── vpn-core/              # 共享业务逻辑（domain models, services）
│   ├── vpn-wireguard/         # WireGuard 数据平面封装
│   ├── vpn-platform/          # 跨平台抽象（TUN / 凭证存储 / daemon）
│   └── vpn-api-types/         # 服务端/客户端共享 API DTO
├── frontend/                   # React 前端
│   ├── src/
│   ├── package.json
│   └── vite.config.ts
├── migrations/                 # sqlx 数据库迁移
├── docker/                     # Dockerfile + docker-compose
├── docs/                       # 项目文档
├── _bmad-output/               # BMad 规划产物（不入 release 镜像）
├── .github/workflows/          # CI 矩阵（Linux/macOS/Windows）
├── README.md
├── Justfile                    # 统一开发命令（just dev / just test / just docker）
└── LICENSE
```

**Development Experience：**

- 后端：`cargo watch -x run` 热重载（开发时）
- 前端：Vite HMR
- 调试：`tracing` 结构化日志 + `RUST_LOG=debug`
- 数据库：`sqlx migrate run` + `sqlx prepare` （offline 模式 CI）
- 跨服务联调：本地 `just dev` 同时启动后端 + 前端 dev server（前端代理到后端）

**Note：** 项目初始化（创建 workspace + 子 crate + Vite 前端 + 第一份 README）应作为第一个 implementation story（Epic 1, Story 1.1）。

## Core Architectural Decisions

### Decision Priority Analysis

**Critical Decisions（阻塞实施，必须立即决定）：**

- Rust 版本与异步运行时（已定 stable + tokio）
- WireGuard 实现（已定 boringtun）
- TUN 库（已定 tun-rs）
- 跨平台 WireGuard 抽象（已定 defguard_wireguard_rs）
- Web 框架（已定 axum）
- 数据库（已定 SQLite via sqlx）
- 认证方案（已定 argon2id + JWT RS256）

**Important Decisions（影响架构形态）：**

- 部署形态（已定 Docker 单容器）
- 前端集成方式（嵌入 vs 独立）
- 客户端 daemon-CLI 通信方式
- 配置管理策略
- 日志与可观测性方案
- 数据库迁移策略

**Deferred Decisions（推迟到 Growth/Vision）：**

- PostgreSQL 切换（保留接口）
- Mesh 拓扑（架构预留扩展点）
- 高可用集群（不在 MVP 视野）
- LDAP/SSO（不在 MVP 视野）
- OpenTelemetry 集成（仅预留 tracing 接口）

### Data Architecture

**数据库选型：**

| 决策 | 选择 | 版本 | 理由 |
|------|------|------|------|
| MVP 数据库 | SQLite | （由 sqlx feature 提供） | 零外部依赖；50 节点完全够用；符合"5 分钟部署"承诺 |
| Growth 数据库 | PostgreSQL | 16+ | 高并发场景；多实例共享 |
| 数据库访问层 | `sqlx` | **0.9.0** | 编译时 SQL 校验；async-first；统一支持 SQLite/Postgres |
| 迁移工具 | `sqlx-cli migrate` | 与 sqlx 同版 | 标准 SQL 文件，无 ORM 锁定 |

**数据访问模式：**

- **Repository Pattern**：业务逻辑层通过 trait 操作（如 `UserRepository`），不直接依赖 sqlx
- 每个聚合根（user, peer, audit_log, session）一个 repo
- 切换 SQLite → PostgreSQL 时仅替换实现，业务代码无变更

**数据建模决策：**

| 数据 | 存储位置 | 决策 |
|------|---------|------|
| 用户账号、Peer 配置、审计日志 | SQLite | 持久化主存储 |
| 限速计数器、登录失败计数 | 内存（HashMap + RwLock） | MVP 单实例；Growth 切 Redis |
| Refresh Token 黑名单 | SQLite（`revoked_tokens` 表） | MVP 单实例；Growth 切 Redis |
| 服务端配置（VPN 网段等） | SQLite + 内存缓存 | 启动加载，变更时刷新缓存 |

**数据验证策略：**

- API 入口：`serde` + `validator` crate 做格式校验
- 业务层：domain model 内部不变量校验（newtype 模式如 `Username(String)`）
- 数据库层：约束 + 触发器兜底（UNIQUE 索引、CHECK 约束）

**缓存策略（MVP）：**

- 仅做最小必要的内存缓存：服务端配置（启动加载）
- 用户/Peer 列表不缓存（SQLite 本地 IO 极快）
- 不引入 Redis 等外部缓存（违反"零外部依赖"承诺）

### Authentication & Security

**密码哈希：**

| 决策 | 选择 | 版本 | 配置 |
|------|------|------|------|
| 算法 | argon2id | `argon2` **0.5.3** | m=64MB, t=3, p=2（OWASP 2024 推荐） |

**Token 体系：**

| 决策 | 选择 | 版本 | 配置 |
|------|------|------|------|
| JWT 库 | `jsonwebtoken` | **10.4.0** | RS256 非对称签名 |
| Access Token | 15 分钟过期 | - | 短期，无服务端状态 |
| Refresh Token | 30 天过期 | - | DB 持久化哈希，支持显式撤销 |
| RS256 密钥 | RSA 2048 | `rsa` crate | 启动时从文件/环境变量加载；不存 DB |

**密钥管理：**

- **JWT 签名密钥**：服务端启动时从环境变量加载；若无则首次启动自动生成并写入持久化路径
- **WireGuard 服务端密钥**：首次启动生成，存入 DB；后续从 DB 读取
- **客户端 WireGuard 私钥**：客户端本地生成，**服务端永不接触**；客户端使用 x25519-dalek **2.0.1** + OsRng

**限速与防爆破：**

| 决策 | 选择 | 版本 | 配置 |
|------|------|------|------|
| 限速中间件 | `tower-governor` | **0.8.0** | GCRA 算法；登录 5/分钟/IP；通用 600/分钟/IP |
| 失败锁定 | 自实现（DB 计数 + 时间窗口） | - | 5 次失败 → 锁 15min；指数退避（15→30→60→120） |

**HTTPS：**

| 决策 | 选择 | 版本 | 理由 |
|------|------|------|------|
| TLS 实现 | `rustls` | **0.23.40** | 纯 Rust，无 OpenSSL 依赖 |
| ACME 自动证书 | `rustls-acme` | **0.15.2** | Axum 集成简单，首次启动自动申请 Let's Encrypt |
| 与 axum 集成 | `tokio-rustls` | **0.26.4** | 流式 TLS |
| 强制 HTTPS | 中间件 | - | 拒绝 HTTP 请求（80 端口重定向到 443） |

**授权（RBAC）：**

- MVP 仅二元角色：`admin` / `user`
- 实现方式：自定义 Axum extractor `RequireAdmin`（提取 JWT claims 校验 role）
- 不引入 Casbin 等重型框架（MVP 不必要）

### API & Communication Patterns

**API 风格：**

| 决策 | 选择 | 版本 |
|------|------|------|
| Web 框架 | `axum` | **0.8.9** |
| API 风格 | RESTful + WebSocket（不用 GraphQL/gRPC） | - |
| 路由组织 | 按版本 + 按资源（`/api/v1/users` / `/api/v1/peers`） | - |
| 数据格式 | JSON（serde_json **1.0.150**） | - |
| WebSocket | axum 内置（`tokio-tungstenite`） | - |

**中间件栈（Tower）：**

| 层级 | 实现 | 版本 | 用途 |
|------|------|------|------|
| `tower-http` | TraceLayer | **0.6.11** | 请求日志 + 请求 ID |
| `tower-http` | CorsLayer | - | 仅生产时按需开启 |
| `tower-http` | CompressionLayer | - | gzip 响应（节省带宽） |
| `tower-governor` | GovernorLayer | **0.8.0** | 限速 |
| 自定义 | AuthLayer | - | JWT 校验 + claims 注入 extension |
| 自定义 | AuditLogLayer | - | 写操作自动记录审计 |

**错误处理：**

- `thiserror` **2.0.18** 定义业务错误（`AppError` enum）
- `anyhow` **1.0.102** 用于 main / startup 错误
- 实现 `IntoResponse` 把 `AppError` 转为统一 JSON 响应（含 code/message/data/timestamp/request_id）

**API 版本化：**

- URL 路径版本化：`/api/v1/...`
- MVP 仅 v1；未来 v2 通过 `/api/v2/...` 并存（不做版本协商）

**WebSocket 设计：**

- 单一端点：`/api/v1/ws/admin/events`
- 单向推送（服务端 → 客户端）
- 事件类型：`PeerJoined` / `PeerLeft` / `PeerUpdated`
- 实现：`tokio::sync::broadcast` channel 在服务端，每个 WS 连接 subscribe

**API 文档（MVP）：**

- 手写 Markdown（在 `docs/api.md`）
- Growth 阶段集成 `utoipa` 自动生成 OpenAPI

**客户端 daemon ↔ CLI 通信：**

| 平台 | 机制 | 路径 |
|------|------|------|
| Linux/macOS | Unix Domain Socket | `/var/run/vpn-cli/daemon.sock`（root）或 `$XDG_RUNTIME_DIR/vpn-cli/daemon.sock`（用户） |
| Windows | Named Pipe | `\\.\pipe\vpn-cli-daemon` |
| 协议 | JSON-RPC 2.0 over framed length-prefix | - |

**客户端 → 服务端通信：**

- HTTPS REST（与浏览器同 API）
- WebSocket（节点心跳上报、配置变更接收）

### Frontend Architecture

**核心库（与 UX Spec 一致）：**

| 决策 | 选择 | 版本 |
|------|------|------|
| 框架 | React | **18.x**（最新稳定） |
| 语言 | TypeScript | **5.x strict** |
| 构建 | Vite | **5.x** |
| UI 库 | antd + @ant-design/pro-components | **5.x** + **2.x** |
| 路由 | react-router-dom | **6.x** |
| HTTP | axios | **1.x** |
| 服务端状态 | @tanstack/react-query | **5.x** |
| 客户端状态 | zustand | **4.x** |
| 时间处理 | dayjs | （AntD 默认） |

**状态管理分工：**

- **React Query**：HTTP 数据（用户列表、节点列表、审计日志、系统信息）
- **Zustand**：WebSocket 实时状态（节点在线状态推送）、UI 全局状态（侧边栏折叠）
- **React Context**：仅用于认证上下文（current user）

**组件架构：**

- Pages（路由级）→ Features（业务组件）→ Components（通用组件）三层
- 自建组件 5 个（见 UX Spec §10）：`<NodeStatusDot>` / `<SetupWizard>` / `<CopyLink>` / `<ConnectionGuide>` / `<EmptyStateWithAction>`

**路由策略：**

- 客户端路由（SPA），不做 SSR
- 路由守卫：未登录跳转 `/login`；非 admin 角色访问 admin 路由跳转 `/`
- 路由级 lazy load（`React.lazy` + `Suspense`）

**Bundle 优化：**

- AntD 5 默认 tree-shaking 友好（无需手动 babel-plugin-import）
- `@ant-design/icons` 按需引入
- 目标：单 chunk < 500KB；首屏总 < 1MB

**前端打包与后端集成：**

| 决策 | 选择 | 理由 |
|------|------|------|
| 集成方式 | **Rust 二进制嵌入静态文件（`rust-embed` 8.11.0）** | 单容器部署；零外部依赖 |
| 路径处理 | 非 `/api/*` 与 `/ws/*` 的请求 → 返回前端静态文件 | SPA fallback 到 index.html |
| 开发模式 | Vite dev server 代理 `/api` 到 axum（localhost:8080） | HMR 友好 |

### Infrastructure & Deployment

**部署形态：**

| 决策 | 选择 | 理由 |
|------|------|------|
| 主要部署方式 | Docker 单容器（`docker run` 一条命令） | 符合"5 分钟部署"承诺 |
| 构建工具 | `cargo-chef` 多阶段 Docker 构建 | 减少 50–75% 构建时间 |
| 容器基础镜像 | `debian:bookworm-slim`（最终）+ `rust:1.82-slim`（构建） | 平衡大小与兼容性 |
| 跨平台编译 | `cross` | 客户端 .deb/.rpm/.pkg/.msi 分发 |
| 替代部署 | systemd 直接安装 | 文档化但非主推 |

**配置管理：**

| 配置 | 存储 | 加载时机 |
|------|------|---------|
| 不变项（端口、绑定地址、DB 路径） | 环境变量 | 启动时 |
| 默认配置 | 编译进二进制（serde defaults） | 启动时 |
| 运行时配置（VPN 网段、admin 设置） | SQLite | 启动时加载，变更时刷新缓存 |
| 密钥（JWT 签名、WG 服务端私钥） | 文件 + 文件权限 600 | 启动时；缺失时自动生成 |

**CI/CD：**

| 阶段 | 工具 | 配置 |
|------|------|------|
| CI | GitHub Actions | 三平台矩阵（Linux/macOS/Windows） |
| 缓存 | `Swatinem/rust-cache@v2` | 减少 50–75% 构建时间 |
| 测试 | `cargo test` + `cargo clippy -D warnings` + `cargo fmt --check` | 每次 PR |
| SQLx 离线 | `SQLX_OFFLINE=true` + 提交 `.sqlx/` | CI 无需数据库 |
| 发布 | `release-plz` 自动 Conventional Commits → CHANGELOG → tag | main 分支合并触发 |
| Docker | 多阶段构建 + GitHub Container Registry | tag 触发推送 |

**Monitoring & Logging：**

| 维度 | MVP 方案 | Growth 增强 |
|------|---------|------------|
| 应用日志 | `tracing` **0.1.44** + `tracing-subscriber` **0.3.23**（JSON 格式输出 stdout） | + ELK/Loki 采集 |
| 指标 | （MVP 不引入） | `metrics` **0.24.6** + `metrics-exporter-prometheus` **0.18.3** |
| 追踪 | request_id 通过 tower-http 注入 | + OpenTelemetry |
| 健康检查 | `GET /health` 返回 200 | - |

**Environment 配置：**

| 环境 | 数据库 | 域名 | 日志级别 |
|------|-------|------|---------|
| 开发 | SQLite `./dev.db` | `localhost:8080`（无 HTTPS） | DEBUG |
| 测试 | SQLite `:memory:`（`sqlx::test`） | - | DEBUG |
| 生产 | SQLite `/var/lib/vpn/vpn.db` | 用户配置（含 ACME） | INFO |

**Scaling Strategy（MVP 不实现，但架构需预留）：**

- 单实例支持 50 节点（MVP）
- 100+ 节点：垂直扩展（更大 VPS）
- 500+ 节点：需要架构升级（PostgreSQL + 横向扩展），不在 MVP 视野

### Decision Impact Analysis

**Implementation Sequence（实施优先级，影响 Epic 排序）：**

1. **Workspace 与基础设施**（先于所有业务代码）
   - `vpn-core`（domain models, errors）
   - `vpn-api-types`（DTO 定义，前后端共享）
   - 数据库 migrations 与 sqlx repos
   - tracing + 错误处理

2. **认证子系统**（其他功能依赖）
   - argon2 密码哈希
   - JWT 签发与校验
   - Tower middleware（Auth + Audit）
   - 限速

3. **WireGuard 数据平面**（独立子系统）
   - `vpn-wireguard` crate
   - `defguard_wireguard_rs` 集成
   - boringtun timer task

4. **服务端 API**（组合上述）
   - REST endpoints（auth, users, peers, admin）
   - WebSocket events
   - SetupWizard 引导接口

5. **客户端**（依赖服务端 API）
   - `vpn-platform` crate（TUN/凭证存储抽象）
   - CLI 命令
   - daemon

6. **前端**（与服务端并行）
   - 项目初始化
   - 自建组件库
   - 8 个页面实现

7. **部署与发布**（最后）
   - Dockerfile + docker-compose
   - GitHub Actions
   - README + 文档

**Cross-Component Dependencies（关键约束）：**

```
vpn-api-types  ← vpn-core ← vpn-server ← (frontend)
                                ↑
                vpn-wireguard ──┘
                                ↑
vpn-platform ← vpn-cli ─────────┘ (复用 vpn-api-types)
```

- `vpn-api-types` 必须最早稳定（一旦发布 v1，向后兼容）
- `vpn-core` 不依赖 axum / sqlx 具体实现，仅依赖 trait（便于测试）
- `vpn-platform` 仅被 `vpn-cli` 使用，不污染服务端代码

## Implementation Patterns & Consistency Rules

### Pattern Categories Defined

识别 **35+ 个潜在 AI 代理冲突点**，覆盖 Rust 后端、TypeScript 前端、跨语言数据契约。所有规则强制执行，CI 集成 lint/format 校验。

### Naming Patterns

#### Rust 命名约定（遵循 Rust 官方规范，强制 `cargo fmt`）

| 元素 | 约定 | 示例 |
|------|------|------|
| Crate 名 | `kebab-case` | `vpn-server`, `vpn-wireguard` |
| Module 名 | `snake_case` | `mod user_repository` |
| Type / Trait / Enum | `PascalCase` | `User`, `PeerStatus`, `UserRepository` |
| Function / Method | `snake_case` | `fn create_user()` |
| Variable / Field | `snake_case` | `let user_id = ...` |
| Constant | `SCREAMING_SNAKE_CASE` | `const MAX_PEERS: usize = 500` |
| Static | `SCREAMING_SNAKE_CASE` | `static CONFIG: OnceLock<...>` |
| Lifetime | 单字符 `'a`, `'b` 或描述性 `'req` | `fn handler<'req>(...)` |
| Generic 类型 | 单字符 `T, K, V` 或描述性 `Repo` | `fn save<R: Repository>(...)` |

#### TypeScript / React 命名约定

| 元素 | 约定 | 示例 |
|------|------|------|
| 组件 (类型) | `PascalCase` | `<UserList />`, `<NodeStatusDot />` |
| 组件文件 | `PascalCase.tsx` | `UserList.tsx`, `NodeStatusDot.tsx` |
| Hook | `camelCase`，前缀 `use` | `useNodeStatus`, `useAuthToken` |
| Hook 文件 | `camelCase.ts` | `useNodeStatus.ts` |
| 工具函数 | `camelCase` | `formatDate`, `parseToken` |
| 工具函数文件 | `camelCase.ts` | `formatDate.ts`, `httpClient.ts` |
| 常量 | `SCREAMING_SNAKE_CASE` | `const API_BASE_URL = ...` |
| Type / Interface | `PascalCase`（不加 `I` 前缀） | `interface User`, `type PeerStatus` |
| Enum | `PascalCase`（值用 `PascalCase`） | `enum Role { Admin, User }` |
| CSS Class | `kebab-case` | `.user-card` |
| 普通目录 | `kebab-case` 或单数名词 | `components/`, `features/users/` |

#### 数据库命名约定

| 元素 | 约定 | 示例 |
|------|------|------|
| 表名 | `snake_case`，复数 | `users`, `peers`, `audit_logs` |
| 列名 | `snake_case` | `user_id`, `created_at`, `last_seen_at` |
| 主键 | 统一 `id`（UUID v7 文本存储） | `id TEXT PRIMARY KEY` |
| 外键 | `<table_singular>_id` | `user_id`, `peer_id` |
| 时间戳 | 后缀 `_at`，类型 `INTEGER`（unix ms） | `created_at`, `updated_at`, `last_seen_at` |
| 布尔列 | 前缀 `is_` 或 `has_` | `is_active`, `has_logged_in` |
| 索引名 | `idx_<table>_<columns>` | `idx_users_email`, `idx_peers_user_id` |
| 唯一约束 | `unq_<table>_<columns>` | `unq_users_username` |
| 外键约束 | `fk_<table>_<column>` | `fk_peers_user_id` |
| 迁移文件 | `YYYYMMDDHHMMSS_<description>.sql` | `20260511_120000_create_users.sql` |

#### API 端点命名约定

| 元素 | 约定 | 示例 |
|------|------|------|
| 资源路径 | `snake_case`，复数名词 | `/api/v1/users`, `/api/v1/audit_logs` |
| 路径参数 | `:param`（axum 语法） | `/api/v1/users/:id` |
| 查询参数 | `snake_case` | `?page_size=20&order_by=created_at` |
| JSON 字段（请求/响应） | `snake_case` | `{ "user_id": "...", "created_at": ... }` |
| HTTP Header | `kebab-case`，X- 前缀仅用于自定义 | `X-Request-Id`, `Authorization` |
| WebSocket 事件类型 | `snake_case` | `{ "type": "peer_joined" }` |
| Error code | 数字 + 业务前缀（PRD §7 已定义） | `1001`（认证错误） |

> **重要：跨语言数据契约 (JSON) 全部 `snake_case`。** 前端 TypeScript 接收后立即转换为 `camelCase`（通过 `transformResponse` 中间件），保持 TS 代码风格统一。

### Structure Patterns

#### Rust 项目结构规则

```
crates/vpn-server/
├── Cargo.toml
├── src/
│   ├── main.rs              # 仅启动逻辑，< 50 行
│   ├── lib.rs               # 重新导出 + app() 工厂函数
│   ├── app.rs               # Axum Router 组装
│   ├── config.rs            # 配置结构 + 加载
│   ├── error.rs             # AppError + IntoResponse
│   ├── state.rs             # AppState（共享给所有 handler）
│   ├── middleware/          # Tower middleware
│   │   ├── mod.rs
│   │   ├── auth.rs
│   │   └── audit.rs
│   ├── handlers/            # HTTP 处理器（按资源分文件）
│   │   ├── mod.rs
│   │   ├── auth.rs
│   │   ├── users.rs
│   │   ├── peers.rs
│   │   └── ws.rs
│   ├── services/            # 业务服务（持有 repo + 编排）
│   │   ├── mod.rs
│   │   ├── auth_service.rs
│   │   ├── user_service.rs
│   │   └── peer_service.rs
│   ├── repositories/        # 数据访问（含 sqlx 查询）
│   │   ├── mod.rs
│   │   ├── user_repo.rs
│   │   └── peer_repo.rs
│   └── domain/              # 共享领域模型（types, traits）
│       ├── mod.rs
│       ├── user.rs
│       └── peer.rs
└── tests/                   # 集成测试
    ├── auth_test.rs
    └── peers_test.rs
```

**关键规则：**

- 每个文件 < 300 行；超过则按子模块拆分
- `handlers/` 仅做：解析请求 → 调用 service → 返回响应，**不写业务逻辑**
- `services/` 包含业务编排（事务、调用多个 repo、触发事件）
- `repositories/` 只做 CRUD；不知道业务规则
- **不**在 `handlers/` 内直接调用 `sqlx::query!`（必须经 repo）
- 单元测试与代码**同文件 `#[cfg(test)] mod tests`**；集成测试在 `tests/` 目录

#### 前端项目结构规则

```
frontend/src/
├── main.tsx                 # 入口
├── App.tsx                  # 路由根 + Provider
├── pages/                   # 路由级页面（按业务）
│   ├── login/
│   │   ├── index.tsx       # 默认导出页面组件
│   │   └── LoginForm.tsx   # 子组件
│   ├── dashboard/
│   ├── users/
│   ├── peers/
│   └── audit-logs/
├── features/                # 业务组件（跨页面共享）
│   ├── auth/
│   │   ├── AuthProvider.tsx
│   │   └── useAuth.ts
│   └── peers/
│       └── PeerStatusBadge.tsx
├── components/              # 通用展示组件（无业务）
│   ├── status/
│   │   └── NodeStatusDot.tsx
│   ├── common/
│   │   ├── CopyLink.tsx
│   │   └── EmptyStateWithAction.tsx
│   └── layout/
│       └── AppLayout.tsx
├── services/                # API 客户端层
│   ├── http.ts             # axios instance + 拦截器
│   ├── auth.api.ts
│   ├── users.api.ts
│   └── peers.api.ts
├── stores/                  # Zustand stores
│   ├── nodeStore.ts
│   └── uiStore.ts
├── hooks/                   # 自定义 Hook
│   ├── useWebSocket.ts
│   └── useDebounce.ts
├── types/                   # TS 类型（与后端 DTO 对齐）
│   ├── user.ts
│   ├── peer.ts
│   └── api.ts
└── utils/                   # 纯函数工具
    ├── format.ts
    └── validators.ts
```

**关键规则：**

- `pages/` 与 `components/` 是**单向依赖**：pages 可以用 components，反之不行
- 每个 page 目录 `index.tsx` 是默认导出
- API 调用必须经 `services/*.api.ts`，**禁止**在组件内直接 `axios.get(...)`
- Zustand store 只放跨页面共享的客户端状态；HTTP 数据走 React Query
- 测试与组件**同文件夹**（`UserList.tsx` + `UserList.test.tsx`）

### Format Patterns

#### API 响应格式（强制统一信封）

**成功响应：**

```json
{
  "code": 0,
  "message": "success",
  "data": { ... },
  "timestamp": 1747000000000,
  "request_id": "01HXG..."
}
```

**错误响应：**

```json
{
  "code": 1001,
  "message": "用户名或密码错误",
  "data": null,
  "timestamp": 1747000000000,
  "request_id": "01HXG..."
}
```

**分页列表响应：**

```json
{
  "code": 0,
  "message": "success",
  "data": {
    "items": [ ... ],
    "total": 234,
    "page": 1,
    "page_size": 20
  },
  "timestamp": ...,
  "request_id": "..."
}
```

**强制规则：**

- 所有 API 响应**必须**走 `AppError::into_response()` / `ApiSuccess::into_response()`
- `code = 0` 仅表示业务成功；HTTP 200 + 业务错误码（如 1001）也是合法响应
- `timestamp` 是 unix milliseconds (i64)
- `request_id` 由 middleware 自动注入（UUID v7 文本）
- 不允许直接返回 `Json(value)`；必须包裹 `ApiResponse`

#### 数据格式标准

| 数据类型 | 格式 | 示例 |
|---------|------|------|
| 时间戳（API） | unix milliseconds (number) | `1747000000000` |
| 时间戳（UI 显示） | 相对时间 / 绝对时间（中文） | `3 分钟前` / `2026-05-11 14:32` |
| 时间戳（DB） | INTEGER (unix ms) | `1747000000000` |
| UUID | UUID v7 文本格式 | `"01HXG..."` |
| IP 地址 | IPv4 字符串 | `"10.8.0.5"` |
| CIDR | 字符串 | `"10.8.0.0/24"` |
| 公钥 | Base64 (urlsafe, no padding) | `"abc...xyz"` |
| Bool | JSON `true`/`false`（DB 用 0/1） | - |
| 空值 | JSON `null`（不用 `""` 或 `0`） | - |
| Enum | 字符串（snake_case） | `"online"`, `"connecting"` |

### Communication Patterns

#### Event 命名（WebSocket）

**格式：** `<resource>_<action>`（snake_case，**过去时**）

| 事件 | 含义 | Payload |
|------|------|---------|
| `peer_joined` | 节点首次连上线 | `{ peer_id, vpn_ip, device_name, user_id }` |
| `peer_left` | 节点离线 | `{ peer_id }` |
| `peer_updated` | 节点 endpoint 变化或心跳 | `{ peer_id, endpoint, last_seen_at }` |
| `user_created` | （Growth）admin 创建用户 | `{ user_id, username }` |
| `user_deleted` | （Growth）用户被删除 | `{ user_id }` |

**WebSocket 消息格式：**

```json
{
  "type": "peer_joined",
  "data": { "peer_id": "...", "vpn_ip": "10.8.0.5", ... },
  "timestamp": 1747000000000
}
```

#### 状态管理模式（前端）

**Zustand store 命名：** `<resource>Store`（camelCase），导出 `use<Resource>Store`

```typescript
// ✅ 标准模式
export const useNodeStore = create<NodeStore>()((set) => ({
  nodes: {},
  updateNode: (node) => set((state) => ({
    nodes: { ...state.nodes, [node.id]: node }
  })),
}));

// ❌ 禁止：直接 mutate
state.nodes[node.id] = node;
```

**React Query 查询键：** 数组形式，按层级

```typescript
// ✅ 标准模式
useQuery({ queryKey: ['users'], ... })
useQuery({ queryKey: ['users', userId], ... })
useQuery({ queryKey: ['users', userId, 'peers'], ... })
useQuery({ queryKey: ['peers', { status: 'online' }], ... })  // 带筛选

// ❌ 禁止：字符串拼接
useQuery({ queryKey: ['users-list'], ... })  // 应改为 ['users']
```

#### CLI 输出模式

**人类可读模式（默认）：**

- 成功用 `✓` (green)
- 失败用 `✗` (red)
- 警告用 `⚠` (yellow)
- 信息用 `ℹ` (blue)
- 颜色通过 `colored` crate；终端不支持时自动降级

**JSON 模式（`--json`）：**

```json
{ "ok": true, "data": { ... } }
{ "ok": false, "error": { "code": 1001, "message": "..." } }
```

### Process Patterns

#### Rust 错误处理模式

**强制约定：**

```rust
// ✅ 服务层 / handler 层使用 AppError
use crate::error::AppError;
type Result<T> = std::result::Result<T, AppError>;

pub async fn create_user(...) -> Result<User> {
    let user = repo.insert(...).await
        .map_err(AppError::Database)?;  // 显式映射
    Ok(user)
}

// ✅ main / 启动代码用 anyhow
fn main() -> anyhow::Result<()> {
    let config = Config::load().context("加载配置失败")?;
    // ...
}

// ❌ 禁止：handler 中用 unwrap / expect
let user = repo.find(...).await.unwrap();  // 编译时 clippy 警告

// ❌ 禁止：把 anyhow::Error 暴露到 API
async fn handler(...) -> anyhow::Result<Json<User>> { ... }  // 应改 AppError
```

**AppError 定义模式（thiserror）：**

```rust
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("用户名或密码错误")]
    InvalidCredentials,           // code 1001

    #[error("Token 过期")]
    TokenExpired,                  // code 1002

    #[error("用户不存在")]
    UserNotFound,                  // code 3001

    #[error("数据库错误")]
    Database(#[from] sqlx::Error), // code 5001（不暴露详情给用户）

    #[error("内部错误")]
    Internal(#[from] anyhow::Error), // code 5003
}
```

#### Loading State 模式（前端）

| 场景 | 模式 |
|------|------|
| 列表/详情首次加载 | React Query 自带 `isLoading` + `<Skeleton>` |
| 按钮提交 | 局部 `loading` state + AntD `<Button loading>` |
| 长任务（> 5s） | 全屏 `<Progress>` 进度条 |
| 静默后台请求 | 不显示 loading（如心跳） |

**禁忌：**

- 不用全屏 `<Spin>` 遮罩（用户不知道在做什么）
- < 100ms 的请求不显示 loading（闪烁干扰）

#### 异步与并发模式（Rust）

**强制约定：**

```rust
// ✅ 全局使用 tokio::sync::*
use tokio::sync::{Mutex, RwLock, broadcast};

// ❌ 禁止：std::sync::Mutex 跨 await（CI clippy 强制检查）
let lock = std_mutex.lock().unwrap();
do_async().await;  // ← 编译警告

// ✅ 锁要尽快释放（不要持有跨 await）
let value = {
    let guard = lock.lock().await;
    guard.value.clone()
};  // 锁已释放
process(value).await;

// ✅ Long-running 任务用 tokio::spawn + JoinHandle
let handle = tokio::spawn(async move { /* ... */ });

// ✅ 后台周期任务用 tokio::time::interval
let mut ticker = tokio::time::interval(Duration::from_secs(30));
loop {
    ticker.tick().await;
    do_periodic_work().await;
}
```

#### 日志模式（tracing）

**强制约定：**

```rust
// ✅ 用 tracing 宏，结构化字段
use tracing::{info, warn, error, instrument};

#[instrument(skip(password))]  // 不记录敏感字段
pub async fn login(username: &str, password: &str) -> Result<Token> {
    info!(username, "用户尝试登录");
    // ...
    if failed {
        warn!(username, reason = "invalid_password", "登录失败");
    }
    todo!()
}

// ❌ 禁止：println! / dbg! 进入主分支
println!("Debug: {}", x);  // CI 强制检查

// ❌ 禁止：日志包含敏感数据
info!("password = {}", password);  // 永远不允许
```

**日志级别约定：**

| 级别 | 用途 |
|------|------|
| `ERROR` | 业务无法继续（数据库连接失败、关键配置缺失） |
| `WARN` | 异常但可继续（登录失败、限速触发、Peer 离线） |
| `INFO` | 关键业务事件（用户登录、Peer 加入、配置变更） |
| `DEBUG` | 详细调试信息（仅 RUST_LOG=debug 时输出） |
| `TRACE` | 极详细（不常用） |

### Enforcement Guidelines

**All AI Agents MUST:**

1. **运行 `cargo fmt && cargo clippy -D warnings`** 在所有 Rust 提交前
2. **使用 `npm run lint && npm run typecheck`** 在所有前端提交前
3. **每个新文件** 遵循上述命名约定（不混 PascalCase/kebab-case）
4. **新 API 端点** 在 `vpn-api-types` crate 定义 DTO，前后端共享
5. **新数据库表/列** 通过 migration 文件创建，不直接改 schema
6. **任何 `pub fn`** 必须有 doc comment（`///`）
7. **任何 `pub` 类型/字段** 必须 derive `Debug`（除非含敏感数据）
8. **任何 API handler** 必须返回 `Result<impl IntoResponse, AppError>`
9. **新增 React 组件** 必须有 TypeScript Props interface
10. **新增 Zustand store** 必须用 immutable update（`set((state) => ...)`）

**Pattern Enforcement：**

| 工具 | 强制检查项 |
|------|----------|
| `rustfmt` | Rust 格式 |
| `clippy -D warnings` | Lint + cross-await-mutex 检查 |
| `cargo-deny` | 依赖版本与许可证 |
| ESLint + Prettier | TypeScript / React 格式与规则 |
| `tsc --noEmit` | 类型严格检查 |
| pre-commit hook | 防止未格式化提交 |
| GitHub Actions | 所有上述强制运行 |

**违反模式时的处理：**

- 代码审查时拒绝合并
- CI 强制失败（不可绕过）
- 更新本模式文档需要团队共识（PR 标 `architecture` 标签）

### Pattern Examples

**Good Example — API Handler：**

```rust
// crates/vpn-server/src/handlers/users.rs
use axum::{Json, extract::State};
use crate::{error::AppError, state::AppState, middleware::RequireAdmin};
use vpn_api_types::{CreateUserRequest, UserResponse, ApiResponse};

#[tracing::instrument(skip(state))]
pub async fn create_user(
    State(state): State<AppState>,
    RequireAdmin(_): RequireAdmin,
    Json(req): Json<CreateUserRequest>,
) -> Result<Json<ApiResponse<UserResponse>>, AppError> {
    let user = state.user_service.create(req).await?;
    Ok(Json(ApiResponse::success(user.into())))
}
```

**Good Example — React Page：**

```typescript
// frontend/src/pages/users/index.tsx
import { ProTable } from '@ant-design/pro-components';
import { fetchUsers } from '@/services/users.api';

export default function UsersPage() {
  return (
    <ProTable<User>
      columns={columns}
      request={async (params) => {
        const data = await fetchUsers(params);
        return { data: data.items, total: data.total, success: true };
      }}
    />
  );
}
```

**Anti-Pattern — 在 Handler 直接写 SQL：**

```rust
// ❌ 禁止
pub async fn create_user(State(pool): State<PgPool>, ...) -> Result<...> {
    sqlx::query!("INSERT INTO users ...").execute(&pool).await?;
    //   ↑ 业务逻辑泄漏到 handler；难以测试与复用
    todo!()
}
```

**Anti-Pattern — 跨 await 持锁：**

```rust
// ❌ 禁止（clippy 报错）
let lock = state.cache.lock().unwrap();
let value = lock.get(key);
let result = expensive_async_op(value).await;  // 锁未释放，会死锁
lock.insert(key, result);
```

**Anti-Pattern — 组件内直接调 axios：**

```typescript
// ❌ 禁止
function UserList() {
  useEffect(() => {
    axios.get('/api/v1/users').then(setUsers);  // 缺乏统一拦截器
  }, []);
}

// ✅ 改为
function UserList() {
  const { data } = useQuery({ queryKey: ['users'], queryFn: fetchUsers });
}
```

## Project Structure & Boundaries

### Complete Project Directory Structure

```
vpn/                                # 仓库根
├── README.md                       # 项目主文档（中文，含 5 分钟 quickstart）
├── LICENSE                         # MIT
├── CHANGELOG.md                    # release-plz 自动生成
├── Cargo.toml                      # Workspace 根配置
├── Cargo.lock                      # 锁定依赖版本（提交到 git）
├── rust-toolchain.toml             # 锁定 Rust 版本（stable, 1.82+）
├── .gitignore
├── .editorconfig
├── Justfile                        # 统一开发命令入口
│
├── crates/                         # ─── Rust 子 crate ────────────────────
│   ├── vpn-api-types/              # 【最早稳定】前后端共享 DTO
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── auth.rs             # LoginRequest, TokenResponse, RefreshRequest
│   │       ├── user.rs             # User, CreateUserRequest, UserResponse, UserListItem
│   │       ├── peer.rs             # Peer, RegisterPeerRequest, PeerResponse, HeartbeatRequest
│   │       ├── audit.rs            # AuditLog, AuditLogQuery
│   │       ├── system.rs           # SystemInfo, SetupRequest
│   │       ├── ws.rs               # WsEvent (peer_joined, peer_left, ...)
│   │       ├── error_codes.rs      # 业务错误码常量
│   │       └── envelope.rs         # ApiResponse, Page<T>
│   │
│   ├── vpn-core/                   # 【共享业务核心】Domain + Trait（无 IO）
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── error.rs            # AppError enum
│   │       ├── domain/             # 领域模型（newtype）
│   │       │   ├── mod.rs
│   │       │   ├── user.rs         # User, Username, Email, Role
│   │       │   ├── peer.rs         # Peer, PeerStatus, VpnIp
│   │       │   ├── session.rs      # Session, RefreshToken
│   │       │   └── audit.rs        # AuditAction, AuditLog
│   │       ├── repository/         # Trait 定义（不含实现）
│   │       │   ├── mod.rs
│   │       │   ├── user_repo.rs
│   │       │   ├── peer_repo.rs
│   │       │   ├── session_repo.rs
│   │       │   └── audit_repo.rs
│   │       ├── service/            # 业务服务 trait
│   │       │   ├── mod.rs
│   │       │   ├── password.rs     # PasswordHasher trait
│   │       │   ├── token.rs        # TokenIssuer trait
│   │       │   └── id.rs           # IdGenerator (UUID v7)
│   │       └── time.rs             # Clock trait（可测试时间）
│   │
│   ├── vpn-wireguard/              # 【数据平面】WireGuard 封装
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── api.rs              # WireGuardApi（基于 defguard_wireguard_rs）
│   │       ├── peer_config.rs      # WgPeerConfig, generate_keypair
│   │       ├── timer.rs            # boringtun update_timers loop
│   │       ├── ip_pool.rs          # CIDR 池 + 静态分配
│   │       └── error.rs            # WgError
│   │
│   ├── vpn-platform/               # 【客户端跨平台】TUN / 凭证 / Daemon
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── tun.rs              # TunDevice trait + 平台实现
│   │       ├── credential.rs       # CredentialStore trait + Keychain/libsecret/Cred Mgr
│   │       ├── daemon.rs           # DaemonRuntime trait + systemd/launchd/Win Svc
│   │       ├── ipc.rs              # Unix Socket / Named Pipe IPC
│   │       └── os.rs               # 平台检测、OS 信息
│   │
│   ├── vpn-server/                 # 【服务端二进制】
│   │   ├── Cargo.toml
│   │   ├── build.rs                # rust-embed: 嵌入 frontend/dist
│   │   ├── src/
│   │   │   ├── main.rs             # < 50 行：load_config → run_server
│   │   │   ├── lib.rs              # pub fn app() 工厂
│   │   │   ├── app.rs              # Axum Router 组装
│   │   │   ├── config.rs           # ServerConfig（env + file + DB）
│   │   │   ├── state.rs            # AppState（共享 service 实例）
│   │   │   ├── error.rs            # AppError → IntoResponse 实现
│   │   │   ├── startup.rs          # 启动校验（数据库、密钥、WireGuard 接口）
│   │   │   ├── shutdown.rs         # 优雅关闭逻辑
│   │   │   ├── middleware/
│   │   │   │   ├── mod.rs
│   │   │   │   ├── auth.rs         # AuthLayer + RequireAdmin extractor
│   │   │   │   ├── audit.rs        # AuditLogLayer
│   │   │   │   ├── request_id.rs   # X-Request-Id 注入
│   │   │   │   └── error.rs        # Panic → 500 响应
│   │   │   ├── handlers/           # HTTP handler（按资源分文件）
│   │   │   │   ├── mod.rs
│   │   │   │   ├── health.rs       # GET /health
│   │   │   │   ├── auth.rs         # POST /auth/login, /refresh, /logout
│   │   │   │   ├── setup.rs        # POST /auth/first-time-setup
│   │   │   │   ├── users.rs        # CRUD /admin/users
│   │   │   │   ├── peers.rs        # POST /peers/register, heartbeat, GET config
│   │   │   │   ├── admin_peers.rs  # GET /admin/peers, DELETE /admin/peers/:id
│   │   │   │   ├── audit.rs        # GET /admin/audit-logs
│   │   │   │   ├── system.rs       # GET /admin/system/info
│   │   │   │   ├── ws.rs           # GET /ws/admin/events
│   │   │   │   └── static_files.rs # SPA fallback（rust-embed）
│   │   │   ├── services/           # 业务服务实现
│   │   │   │   ├── mod.rs
│   │   │   │   ├── auth_service.rs       # 登录、改密、Token 签发
│   │   │   │   ├── user_service.rs       # 用户 CRUD + 级联清理
│   │   │   │   ├── peer_service.rs       # Peer 注册/心跳/下线 + WG 集成
│   │   │   │   ├── audit_service.rs      # 写审计日志
│   │   │   │   ├── system_service.rs     # 服务端配置管理
│   │   │   │   ├── password_hasher.rs    # PasswordHasher impl（argon2id）
│   │   │   │   ├── token_issuer.rs       # TokenIssuer impl（jsonwebtoken RS256）
│   │   │   │   ├── id_generator.rs       # UUID v7 实现
│   │   │   │   └── event_bus.rs          # tokio broadcast PeerEvent
│   │   │   ├── repositories/       # sqlx 实现
│   │   │   │   ├── mod.rs
│   │   │   │   ├── user_repo_sqlite.rs
│   │   │   │   ├── peer_repo_sqlite.rs
│   │   │   │   ├── session_repo_sqlite.rs
│   │   │   │   ├── audit_repo_sqlite.rs
│   │   │   │   └── setup_repo_sqlite.rs  # admin 存在检查
│   │   │   ├── wireguard_runtime/  # 服务端 WireGuard 运行时（持有 WgApi）
│   │   │   │   ├── mod.rs
│   │   │   │   ├── runtime.rs      # 启动 WG 接口、加载已有 peers
│   │   │   │   └── sync.rs         # service → wireguard 配置同步
│   │   │   ├── tls/                # ACME 证书管理
│   │   │   │   ├── mod.rs
│   │   │   │   └── acme.rs         # rustls-acme 集成
│   │   │   ├── auth/
│   │   │   │   ├── mod.rs
│   │   │   │   ├── claims.rs       # JWT Claims
│   │   │   │   └── extractor.rs    # CurrentUser, RequireAdmin
│   │   │   └── ratelimit/
│   │   │       ├── mod.rs
│   │   │       └── login.rs        # 登录失败计数
│   │   └── tests/                  # 集成测试
│   │       ├── common.rs           # 测试 fixtures（启动 server, mock 数据）
│   │       ├── auth_test.rs
│   │       ├── users_test.rs
│   │       ├── peers_test.rs
│   │       └── setup_test.rs
│   │
│   └── vpn-cli/                    # 【客户端二进制】
│       ├── Cargo.toml
│       └── src/
│           ├── main.rs             # clap 入口
│           ├── lib.rs
│           ├── cli/                # CLI 命令实现
│           │   ├── mod.rs
│           │   ├── login.rs        # vpn-cli login
│           │   ├── logout.rs       # vpn-cli logout
│           │   ├── start.rs        # vpn-cli start
│           │   ├── stop.rs         # vpn-cli stop
│           │   ├── status.rs       # vpn-cli status
│           │   ├── config.rs       # vpn-cli config
│           │   └── daemon.rs       # vpn-cli daemon（前台模式）
│           ├── daemon/             # daemon 内部逻辑
│           │   ├── mod.rs
│           │   ├── service.rs      # 主循环（接收 CLI 指令 + 维护隧道）
│           │   ├── tunnel.rs       # boringtun + tun-rs 集成
│           │   ├── reconnect.rs    # 指数退避自动重连
│           │   └── network_watch.rs # 网络变化检测
│           ├── api_client/         # 服务端 API 客户端（基于 reqwest）
│           │   ├── mod.rs
│           │   ├── client.rs
│           │   ├── auth.rs
│           │   ├── peers.rs
│           │   └── ws.rs           # WebSocket 客户端
│           ├── config.rs           # 客户端配置文件读写
│           ├── ipc/                # CLI ↔ daemon IPC
│           │   ├── mod.rs
│           │   ├── server.rs       # daemon 侧 IPC 服务
│           │   ├── client.rs       # CLI 侧 IPC 客户端
│           │   └── protocol.rs     # JSON-RPC schema
│           └── output.rs           # 终端输出（人类 / JSON 模式切换）
│
├── frontend/                       # ─── React 前端 ──────────────────────
│   ├── package.json
│   ├── package-lock.json
│   ├── tsconfig.json
│   ├── vite.config.ts              # 含 /api proxy 开发配置
│   ├── eslint.config.js
│   ├── index.html
│   ├── public/                     # 静态资源
│   │   └── favicon.ico
│   └── src/
│       ├── main.tsx                # React 入口
│       ├── App.tsx                 # 路由根
│       ├── theme.ts                # AntD theme tokens（geekblue 等）
│       ├── pages/                  # 路由级页面
│       │   ├── setup/              # 首次部署引导（公开）
│       │   ├── login/
│       │   ├── dashboard/
│       │   ├── users/
│       │   ├── peers/
│       │   ├── audit-logs/
│       │   ├── account/
│       │   └── connect/            # 员工 setup 公开页 /setup（员工端）
│       ├── features/               # 业务组件
│       │   ├── auth/
│       │   └── peers/
│       ├── components/             # 通用组件
│       │   ├── status/
│       │   │   └── NodeStatusDot.tsx
│       │   ├── common/
│       │   │   ├── CopyLink.tsx
│       │   │   └── EmptyStateWithAction.tsx
│       │   └── layout/
│       │       ├── AppLayout.tsx    # ProLayout 包装
│       │       └── UserMenu.tsx
│       ├── services/               # API 客户端
│       │   ├── http.ts             # axios + 拦截器
│       │   ├── auth.api.ts
│       │   ├── users.api.ts
│       │   ├── peers.api.ts
│       │   ├── audit.api.ts
│       │   ├── system.api.ts
│       │   └── ws.ts               # WebSocket 连接管理
│       ├── stores/                 # Zustand
│       │   ├── nodeStore.ts        # 节点实时状态
│       │   └── uiStore.ts          # 侧边栏折叠等
│       ├── hooks/
│       │   ├── useWebSocket.ts
│       │   ├── useDebounce.ts
│       │   └── useOsDetect.ts      # /setup 页 OS 检测
│       ├── types/                  # TS 类型（与 vpn-api-types 对齐）
│       └── utils/
│           ├── format.ts           # 时间格式化（"3 分钟前"）
│           ├── validators.ts
│           ├── caseConvert.ts      # snake_case ↔ camelCase
│           └── constants.ts
│
├── migrations/                     # ─── sqlx 数据库迁移 ─────────────────
│   ├── 20260511_120000_init.sql
│   ├── 20260511_120100_users.sql
│   ├── 20260511_120200_peers.sql
│   ├── 20260511_120300_sessions.sql
│   ├── 20260511_120400_audit_logs.sql
│   └── 20260511_120500_system_config.sql
│
├── .sqlx/                          # sqlx 离线查询缓存（提交到 git）
│
├── docker/                         # ─── Docker 部署 ─────────────────────
│   ├── Dockerfile                  # 多阶段：planner + builder + runtime
│   ├── docker-compose.yml          # 单服务
│   ├── docker-compose.test.yml     # E2E 测试用（server + 多 client）
│   └── entrypoint.sh
│
├── installers/                     # ─── 客户端安装包构建 ────────────────
│   ├── macos/build-pkg.sh
│   ├── windows/build-msi.wxs
│   └── linux/{build-deb.sh, build-rpm.sh}
│
├── docs/                           # ─── 文档 ────────────────────────────
│   ├── README.md
│   ├── deployment.md               # 部署指南（含截图）
│   ├── api.md                      # API 参考
│   ├── client-cli.md               # CLI 用户手册
│   ├── troubleshooting.md          # 故障排查
│   └── images/                     # 截图资源
│
├── _bmad-output/                   # ─── BMad 规划产物（开发期，不打包）─
│   └── planning-artifacts/{prd,ux-design-specification,architecture,epics}.md
│
├── .github/                        # ─── CI/CD ──────────────────────────
│   ├── workflows/{ci,release,docker}.yml
│   ├── dependabot.yml
│   └── ISSUE_TEMPLATE/
│
└── scripts/                        # 开发辅助脚本
    ├── setup-dev.sh
    ├── seed-db.sh
    └── release.sh
```

### Architectural Boundaries

**API Boundaries（HTTP/WebSocket）：**

- **公开端点**（无需认证）：`POST /auth/login`、`POST /auth/first-time-setup`（仅 admin 未存在）、`GET /setup`（前端页）、`GET /health`、`GET /api/v1/public/*`（OS 检测、下载链接）
- **认证端点**（JWT 必需）：`/api/v1/auth/refresh`、`/api/v1/auth/logout`、`/api/v1/auth/change-password`、`/api/v1/peers/me/*`
- **Admin 端点**（JWT + admin 角色）：`/api/v1/admin/*`、`/api/v1/ws/admin/events`
- **静态资源**：所有非 `/api/*` 和 `/ws/*` 的 GET 请求 → SPA fallback（rust-embed 服务 `index.html`）

**Component Boundaries（Crate 间）：**

```
┌─────────────────────────────────────────────────────────────┐
│                    vpn-api-types                            │
│     （前后端共享 DTO；最早稳定；零业务逻辑；无 IO）           │
└─────────────────────────────────────────────────────────────┘
            ↑                                ↑
            │                                │
┌───────────┴──────────┐         ┌───────────┴──────────┐
│      vpn-core         │         │     vpn-platform     │
│   （Domain + Trait    │         │  （客户端跨平台抽象） │
│    无具体 IO 实现）   │         │  TUN / Cred / Daemon │
└───────────┬──────────┘         └───────────┬──────────┘
            ↑                                ↑
   ┌────────┴───────────┐                    │
   │                    │                    │
┌──┴────────┐  ┌────────┴──────┐  ┌──────────┴──────┐
│ vpn-      │  │  vpn-server   │  │   vpn-cli       │
│ wireguard │←─│  (binary)     │  │   (binary)      │
└───────────┘  └───────────────┘  └─────────────────┘
                                  ┌──────────────────┐
                                  │     frontend     │
                                  │   (Vite SPA)     │
                                  │  与 vpn-server   │
                                  │  通过 HTTP/WS    │
                                  └──────────────────┘
```

**强制边界规则：**

- `vpn-api-types` **不依赖任何 IO crate**（无 sqlx, axum, reqwest）；纯 serde 结构
- `vpn-core` **不依赖具体实现 crate**（无 sqlx, axum）；只定义 trait
- `vpn-server` 是 `vpn-core` trait 的具体实现者
- `vpn-cli` **不依赖 `vpn-wireguard`**（客户端只用 boringtun + tun-rs，不需要服务端的 ip pool 等）
- frontend 与 vpn-server **仅通过 HTTP/WebSocket** 通信，无其他耦合

**Service Boundaries（vpn-server 内部）：**

```
HTTP Handler  →  Service  →  Repository  →  SQLite
              ↘  Service  →  WireGuardApi  →  Kernel/WinTUN
              ↘  EventBus（broadcast）→  WebSocket connections
```

- Handler 仅做请求解析与响应封装
- Service 持有多个 Repo + WireGuard API + EventBus，编排业务流程（含事务）
- Repository 仅做 CRUD，不知业务规则
- 跨 service 调用通过 `AppState` 拿引用，**禁止 service 间循环依赖**

**Data Boundaries：**

- **持久化数据**（SQLite）：users, peers, sessions, audit_logs, system_config, revoked_tokens
- **内存状态**（Arc<RwLock>）：限速计数器、当前活跃 WebSocket 连接列表
- **WireGuard 内核态**：peer 配置（启动时从 SQLite 同步加载）
- **客户端本地**：WireGuard 私钥（加密文件）、Refresh Token（OS Keychain/Credential Mgr）

### Requirements to Structure Mapping

**FR 能力域 → 代码位置：**

| FR 能力域 | 关键代码位置 |
|-----------|------------|
| A. 部署与初始化（FR1-5） | `vpn-server/src/startup.rs` + `tls/acme.rs` + `handlers/setup.rs` + `wireguard_runtime/runtime.rs`（重启恢复） |
| B. 账号与认证（FR6-13） | `vpn-server/src/services/auth_service.rs` + `password_hasher.rs` + `token_issuer.rs` + `middleware/auth.rs` + `ratelimit/login.rs` |
| C. 用户管理（FR14-20） | `vpn-server/src/handlers/users.rs` + `services/user_service.rs` + `repositories/user_repo_sqlite.rs` + frontend `pages/users/` |
| D. 节点管理（FR21-29） | `vpn-server/src/handlers/peers.rs` + `admin_peers.rs` + `services/peer_service.rs` + `wireguard_runtime/sync.rs` + frontend `pages/peers/` |
| E. 客户端体验（FR30-40） | `vpn-cli/src/cli/*` + `daemon/*` + `vpn-platform/src/{tun,credential,daemon,ipc}.rs` + frontend `pages/connect/` |
| F. 隧道运维（FR41-47） | `vpn-cli/src/daemon/{tunnel,reconnect,network_watch}.rs` + `vpn-wireguard/src/timer.rs` |
| G. 审计与可见性（FR48-55） | `vpn-server/src/middleware/audit.rs` + `services/audit_service.rs` + `handlers/audit.rs` + `handlers/ws.rs` + frontend `pages/audit-logs/` |
| H. 安全防护（FR56-63） | `vpn-server/src/middleware/auth.rs` + `ratelimit/login.rs` + `tls/acme.rs` + `services/user_service.rs`（级联删除） |

**关键 NFR → 实现位置：**

| NFR | 实现位置 |
|-----|---------|
| NFR-R7（WG timer ≤ 250ms） | `vpn-wireguard/src/timer.rs`：独立 tokio task + interval 100ms |
| NFR-R5（重启恢复） | `vpn-server/src/wireguard_runtime/runtime.rs::reload_all_peers()` |
| NFR-S8（非 root + CAP_NET_ADMIN） | `docker/Dockerfile` USER + `installers/linux/systemd.service` |
| NFR-O1（5 分钟部署） | `docker/Dockerfile` + `tls/acme.rs` + `handlers/setup.rs` |
| NFR-C5（跨平台对等） | `vpn-platform/src/tun.rs` 三平台实现 + CI 矩阵测试 |

### Integration Points

**Internal Communication：**

- **vpn-server 内部**：HTTP → handler → service → repo（线性）；service 间通过 EventBus 解耦（broadcast）
- **vpn-cli 内部**：CLI 命令 → IPC client → daemon 接收 → 调用 daemon service
- **CLI ↔ daemon**：Unix Socket / Named Pipe + JSON-RPC 2.0

**External Integrations：**

- **ACME（Let's Encrypt）**：服务端首次启动调用，证书过期自动续期（rustls-acme 处理）
- **WireGuard 内核接口**：通过 defguard_wireguard_rs 抽象
- **OS 凭据库**：客户端通过 keyring crate（macOS Keychain / Linux libsecret / Windows Credential Manager）
- **GitHub Releases**：CI 自动构建二进制 + Docker 镜像

**Data Flow（管理员创建用户）：**

```
用户操作（浏览器）
  → axios → /api/v1/users (HTTPS)
  → AuthLayer 校验 JWT
  → AuditLogLayer 记录
  → handlers/users.rs
  → user_service.create()
    → password_hasher.hash()
    → user_repo.insert() (SQLite)
    → audit_service.log()
  → ApiResponse::success(user_response)
  → HTTP 200
  → axios 响应拦截器解包
  → React Query 更新缓存
  → ProTable 重新渲染
```

**Data Flow（客户端连接）：**

```
vpn-cli login
  → IPC → daemon
  → api_client/auth.rs (POST /auth/login)
  → vpn-server handlers/auth.rs
  → 返回 Token
  → daemon 保存 Refresh Token 到 Keychain
  → daemon 生成 WG 密钥对（x25519-dalek）
  → POST /peers/register（带公钥 + JWT）
  → vpn-server peer_service.register()
    → peer_repo.insert()
    → wireguard_runtime/sync.rs：调用 WgApi.configure_peer()
    → event_bus.publish(PeerJoined)
  → 返回服务端 endpoint + 虚拟 IP
  → daemon 启动 boringtun Tunn
  → daemon 启动 tun-rs 接口
  → daemon 启动 timer task（每 100ms）
  → daemon 启动主循环：TUN ↔ UDP
```

### File Organization Patterns

**Configuration Files：**

| 文件 | 用途 |
|------|------|
| `Cargo.toml` | Rust workspace + 依赖版本 |
| `rust-toolchain.toml` | 锁定 Rust 版本 |
| `Justfile` | 开发命令入口（`just dev`, `just test`, `just docker`） |
| `frontend/vite.config.ts` | Vite 配置（含 /api 代理） |
| `frontend/tsconfig.json` | TypeScript strict |
| `.editorconfig` | 跨编辑器一致 |

**Source Organization：**

- 后端按 crate 隔离边界，crate 内按层（handler/service/repo）分层
- 前端按 pages → features → components 分层，强单向依赖

**Test Organization：**

| 测试类型 | 位置 |
|---------|------|
| Rust 单元测试 | 同文件 `#[cfg(test)] mod tests` |
| Rust 集成测试 | `crates/vpn-server/tests/*.rs` |
| Rust E2E | `docker/docker-compose.test.yml` + `tests/e2e/` |
| 前端单元测试 | 同目录 `*.test.tsx` |
| 前端 E2E | 未做（MVP 排除） |

**Asset Organization：**

- 前端静态资源在 `frontend/public/`
- 生产构建到 `frontend/dist/`
- `vpn-server/build.rs` 用 `rust-embed` 把 `frontend/dist/` 嵌入二进制

### Development Workflow Integration

**Development Server Structure：**

```bash
# 终端 1: 后端
just dev-server   # cargo watch -x 'run --bin vpn-server'

# 终端 2: 前端
just dev-frontend # cd frontend && npm run dev (Vite 代理到 localhost:8080)

# 或一条命令同时启动
just dev          # 使用 concurrently
```

**Build Process Structure：**

```bash
just build        # 1. cd frontend && npm run build
                  # 2. cargo build --release (build.rs 嵌入 frontend/dist)
                  # 3. 输出: target/release/vpn-server + vpn-cli

just docker       # 1. docker build -f docker/Dockerfile -t vpn:latest .
                  # 2. cargo-chef 多阶段缓存
                  # 3. 最终镜像 ~80MB（包含 frontend）

just installer-mac    # 客户端 .pkg
just installer-win    # 客户端 .msi
just installer-linux  # 客户端 .deb / .rpm
```

**Deployment Structure：**

- 单容器部署：`docker run -p 443:8443 -v vpn-data:/var/lib/vpn vpn:latest`
- systemd 直装：拷贝二进制到 `/usr/local/bin/vpn-server`，配置 service 文件
- 客户端：用户从 GitHub Releases 下载对应平台安装包

## Architecture Validation Results

### Coherence Validation ✅

**Decision Compatibility：**

所有技术选择经过版本兼容性核实（2026-05-22 crates.io 实时数据），无冲突：

- `tokio 1.52.3` + `axum 0.8.9` + `tower 0.5.3` + `tower-http 0.6.11` + `tower-governor 0.8.0`：同生态系列，官方维护，互相兼容
- `sqlx 0.9.0` + `tokio` + SQLite 驱动：async-first，原生支持
- `boringtun 0.7.1` + `tun-rs 2.8.3` + `defguard_wireguard_rs 0.9.6`：均为活跃维护的纯 Rust crate
- `rustls 0.23.40` + `tokio-rustls 0.26.4` + `rustls-acme 0.15.2`：同 rustls 生态
- `argon2 0.5.3` + `jsonwebtoken 10.4.0`：RustCrypto 生态
- 前端 React 18 + AntD 5 + ProComponents 2：官方推荐组合，UX Spec 已验证

**Pattern Consistency：**

- 命名约定一致：Rust 全栈 `snake_case` / TypeScript 全栈 `camelCase` / API JSON 全 `snake_case`
- 错误处理一致：`thiserror::AppError` → `IntoResponse` → 统一 `ApiResponse` 信封
- 异步模式一致：全局 `tokio::sync::*`，clippy 强制检查防止 `std::sync::Mutex` 跨 await
- 日志一致：`tracing` 结构化字段 + 5 级日志规范

**Structure Alignment：**

- 6 个 Rust crate 边界与依赖图严格单向（`vpn-api-types` 在最底层，`vpn-server` / `vpn-cli` 在最顶层）
- 前端 pages → features → components 单向依赖
- 服务端 handler → service → repository 三层架构清晰
- 跨平台抽象隔离在 `vpn-platform` crate，不污染其他代码

### Requirements Coverage Validation ✅

**Functional Requirements Coverage（49 项 MVP FR 100% 映射）：**

| FR 能力域 | FR 数 | 架构覆盖位置 | 状态 |
|----------|-------|-----------|------|
| A. 部署与初始化（FR1-5） | 5/5 | `vpn-server/{startup, tls/acme, handlers/setup, wireguard_runtime}` | ✅ |
| B. 账号与认证（FR6-12） | 7/7 | `services/{auth_service, password_hasher, token_issuer}` + `middleware/auth` + `ratelimit/login` | ✅ |
| C. 用户管理（FR14-20） | 7/7 | `handlers/users` + `services/user_service` + `repositories/user_repo_sqlite` + frontend `pages/users/` | ✅ |
| D. 节点管理（FR21-29） | 9/9 | `handlers/{peers, admin_peers}` + `services/peer_service` + `wireguard_runtime/sync` | ✅ |
| E. 客户端体验（FR30-38） | 9/9 | `vpn-cli/{cli/*, daemon/*}` + `vpn-platform/{tun, credential, daemon, ipc}` + frontend `pages/connect/` | ✅ |
| F. 隧道运维（FR41-45） | 5/5 | `vpn-cli/daemon/{tunnel, reconnect, network_watch}` + `vpn-wireguard/timer` | ✅ |
| G. 审计与可见性（FR48-52） | 5/5 | `middleware/audit` + `services/audit_service` + `handlers/{audit, ws}` + frontend `pages/audit-logs/` | ✅ |
| H. 安全防护（FR56-61） | 6/6 | `middleware/auth` + `ratelimit/login` + `tls/acme` + `services/user_service`（级联清理） | ✅ |

**MVP 范围合计：49/49 FR 全部映射到具体代码位置，无遗漏。**

**Growth & Vision FR（架构预留扩展点）：**

| FR | Growth/Vision | 架构预留 |
|----|--------------|---------|
| FR13（RBAC 三角色） | G | `middleware/auth.rs` 的 extractor 可扩展为支持多角色 |
| FR39（Tauri GUI） | G | 客户端 daemon API 已通过 IPC 暴露，GUI 可直接复用 |
| FR40（移动端） | V | API 已 RESTful，移动端直接调用 |
| FR46-47（Mesh P2P + NAT 穿透） | V | 架构未禁止，但需要新 crate `vpn-mesh` 与新的 service |
| FR53-55（流量统计/WS/Prometheus） | G | `event_bus.rs` 已就位，扩展 `services/traffic_service` 即可 |
| FR62（IP 白名单） | G | 通过新 middleware 实现 |
| FR63（LDAP/SSO） | V | 新增 `services/sso_service` + auth 流程扩展 |

**Non-Functional Requirements Coverage（38 项 MVP NFR 100% 覆盖）：**

| 类别 | NFR 数 | 关键实现 | 状态 |
|------|-------|---------|------|
| Performance（NFR-P1~P6） | 6/6 | boringtun userspace + tokio multi-thread + 单进程低开销 | ✅ |
| Security（NFR-S1~S11） | 11/11 | argon2id + RS256 + tower-governor + rustls + 非 root + capabilities | ✅ |
| Reliability（NFR-R1~R7） | 7/7 | 持久化 + 重启恢复 + tokio task 100ms timer + 优雅关闭 | ✅ |
| Scalability（NFR-SC1, SC2） | 2/2 | tokio 异步 + SQLite 本地 IO 极快 | ✅ |
| Compatibility（NFR-C1~C6） | 6/6 | tun-rs 跨平台 + CI 三平台矩阵 + 浏览器现代标准 | ✅ |
| Operability（NFR-O1~O7） | 7/7 | Docker 单容器 + ACME 自动 + 嵌入式前端 + 明确错误 | ✅ |
| Maintainability（NFR-M1~M5） | 5/5 | clippy + sqlx test + CI 矩阵 + 70% 覆盖率目标 | ✅ |

**UX 用户旅程支撑（PRD §5 → UX §9 → 架构 §结构）：**

| 用户旅程 | 架构支撑 |
|---------|---------|
| J1.1 首次部署 | rustls-acme + SetupWizard + first-time-setup endpoint |
| J1.2 添加新员工 | user_service + audit_service + EventBus + ShareLinkModal |
| J1.3 故障排查 | audit_service + login_fail_logs + peer status + admin_peers |
| J2.1 员工首次接入 | OS detect + 多平台安装包 + CLI login + IPC + boringtun |
| J2.2 断线自动恢复 | reconnect.rs（指数退避） + network_watch.rs |

### Implementation Readiness Validation ✅

**Decision Completeness：**

- 所有 critical 决策已锁定具体版本（核实于 2026-05-22 crates.io）
- 35+ 实施模式规则覆盖命名/结构/格式/通信/流程
- 每个模式都附有 Good Example + Anti-Pattern 代码示例
- CI 强制工具栈（rustfmt + clippy + ESLint + tsc）确保规则不被违反

**Structure Completeness：**

- 6 个 Rust crate + frontend + 完整目录树定义（含每个文件用途说明）
- 数据库迁移文件命名规范明确
- Docker + installers + docs + .github 周边目录完整
- Justfile 统一开发命令入口

**Pattern Completeness：**

- 命名冲突点已覆盖：Rust / TS / DB / API 四套规范
- 通信冲突点已覆盖：API 信封 / WS 事件 / IPC 协议 / CLI 输出
- 流程冲突点已覆盖：错误处理 / 异步并发 / 日志 / Loading 状态
- 强制执行机制明确：10 条"All AI Agents MUST" + 工具链表

### Gap Analysis Results

**Critical Gaps（阻塞实施）：** 无

**Important Gaps（应在 Epic 阶段补充）：**

1. **数据库 Schema 详细定义** — 架构文档列出了表名和迁移文件命名，但没有给出每个表的完整 CREATE TABLE SQL。建议在 Epic 1 的 Story "数据库 schema 初始化" 中详细定义
2. **WireGuard timer 与 peer 操作的并发模型细节** — 架构指出用 `Arc<Mutex<Tunn>>` 但未给出具体的 actor / channel 设计。建议在 Epic 3（WireGuard 集成）的 Story 中详细设计
3. **客户端 daemon 与 CLI 的 JSON-RPC schema** — 已定义协议但未列出具体 method 与 params。建议在 Epic 4 的 Story 中输出完整 schema 文档
4. **错误码完整清单** — PRD §7 给出分类（1xxx-5xxx），架构 §错误处理给出示例，但未列出所有具体错误码。建议在 Epic 1 的 `vpn-api-types::error_codes` 模块中定义

**Minor Gaps（推迟到 Growth）：**

1. **Prometheus 指标接口** — Growth 阶段引入，MVP 不必要
2. **OpenAPI 自动生成** — Growth 阶段引入 `utoipa`，MVP 用 Markdown
3. **Storybook 组件文档** — Growth 阶段引入，MVP 用 Markdown
4. **OpenTelemetry 集成** — Growth/Vision 阶段
5. **PostgreSQL 切换具体迁移脚本** — Growth 阶段，架构已留接口

### Validation Issues Addressed

**已主动规避的常见架构陷阱：**

1. ✅ **boringtun timer 静默断线**：明确独立 tokio task + 100ms interval 设计（NFR-R7）
2. ✅ **`std::sync::Mutex` 跨 await 死锁**：架构规定全局 `tokio::sync::*` + clippy 强制
3. ✅ **`Tunn` 非 Send**：使用 `Arc<Mutex<Tunn>>` 或 actor + channel
4. ✅ **跨平台 TUN 差异**：`defguard_wireguard_rs` 统一抽象 + `vpn-platform` 隔离
5. ✅ **SQLx 离线 CI 失败**：`SQLX_OFFLINE=true` + 提交 `.sqlx/`
6. ✅ **Token 撤销**：Refresh Token 在 SQLite `revoked_tokens` 表显式管理
7. ✅ **前端 snake_case ↔ camelCase**：axios 拦截器统一转换，TS 代码风格一致

### Architecture Completeness Checklist

**Requirements Analysis**

- [x] Project context thoroughly analyzed
- [x] Scale and complexity assessed
- [x] Technical constraints identified
- [x] Cross-cutting concerns mapped

**Architectural Decisions**

- [x] Critical decisions documented with versions
- [x] Technology stack fully specified
- [x] Integration patterns defined
- [x] Performance considerations addressed

**Implementation Patterns**

- [x] Naming conventions established
- [x] Structure patterns defined
- [x] Communication patterns specified
- [x] Process patterns documented

**Project Structure**

- [x] Complete directory structure defined
- [x] Component boundaries established
- [x] Integration points mapped
- [x] Requirements to structure mapping complete

**16/16 全部勾选。**

### Architecture Readiness Assessment

**Overall Status：READY FOR IMPLEMENTATION**

**Confidence Level：High**

判断依据：

- 16/16 checklist 项全部通过
- 无 Critical Gap
- 所有 MVP FR/NFR 100% 映射到具体代码位置
- 所有技术选择经版本兼容性核实
- 所有已知技术陷阱已主动规避

**Key Strengths：**

1. **决策密度高**：35+ 实施模式规则覆盖 AI 代理可能的所有冲突点
2. **依赖图清晰**：6 个 Rust crate 严格单向依赖，无循环
3. **跨平台抽象优雅**：`vpn-platform` crate 隔离差异，不污染业务代码
4. **可扩展性**：Growth/Vision FR 都有明确扩展点（EventBus、middleware、新 crate）
5. **可测试性**：Repository pattern + trait 抽象 + sqlx::test + 独立 service 服务层
6. **运维友好**：单容器部署 + ACME 自动 + 嵌入式前端 + 明确错误
7. **基于研究的务实选择**：不引入未验证的库（boringtun、defguard_wireguard_rs 已生产验证）

**Areas for Future Enhancement：**

1. **观测性**：Phase 2 引入 Prometheus 指标 + OpenTelemetry 追踪
2. **多实例 HA**：Phase 3 重新评估架构（共享 PostgreSQL + Redis）
3. **Mesh P2P**：Phase 3 引入 `vpn-mesh` crate + NAT 穿透
4. **客户端 GUI**：Phase 2 用 Tauri 复用 React 前端代码
5. **API 文档自动化**：Phase 2 引入 `utoipa`

### Implementation Handoff

**AI Agent Guidelines：**

1. **严格遵循已锁定版本** — `Cargo.toml` 中所有依赖版本不可随意升级；如需升级，在 PR 中说明理由
2. **不偏离 `vpn-api-types` 契约** — 所有 API 端点的 request/response 必须定义在此 crate；前端从此 crate 复制 TS 类型
3. **跨语言数据契约保持 `snake_case`** — 不在后端做 camelCase 转换；前端入口 axios 拦截器统一转换
4. **错误处理走 AppError → ApiResponse 链路** — 不直接 `Json::from(...)`；不暴露 `anyhow::Error` 到 HTTP
5. **数据库变更走 migration 文件** — 不直接 ALTER TABLE；migration 文件不可修改已发布版本
6. **新增功能时先看 §结构 → 找到对应文件位置** — 不创建未在结构中定义的目录
7. **遇到不确定时优先选择"最简实现"** — MVP 阶段拒绝过度工程
8. **所有 PR 必须通过 CI**（rustfmt + clippy + tests + 三平台矩阵）

**First Implementation Priority：**

```bash
# Epic 1, Story 1.1: 项目初始化
mkdir vpn && cd vpn
cargo new --vcs git .

# 配置 Cargo.toml 作为 workspace 根（包含 [workspace.dependencies]）
# 创建 6 个子 crate
cargo new --bin crates/vpn-server
cargo new --bin crates/vpn-cli
cargo new --lib crates/vpn-core
cargo new --lib crates/vpn-wireguard
cargo new --lib crates/vpn-platform
cargo new --lib crates/vpn-api-types

# 初始化前端
npm create vite@latest frontend -- --template react-ts
cd frontend
npm install antd @ant-design/pro-components @ant-design/icons \
  @tanstack/react-query zustand axios react-router-dom dayjs

# 初始化 Justfile、README.md、.gitignore、.editorconfig、rust-toolchain.toml
# 配置 GitHub Actions CI 矩阵
```
