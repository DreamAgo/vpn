---
stepsCompleted: [step-01-validate-prerequisites, step-02-design-epics, step-03-create-stories, step-04-final-validation]
status: complete
inputDocuments:
  - prd.md
  - ux-design-specification.md
  - architecture.md
---

# vpn - Epic Breakdown

## Overview

本文档提供 vpn 项目的完整 Epic 与 Story 拆解，将 PRD（63 FR + 43 NFR）、UX 设计规范（自建组件 + UX 设计需求）与 Architecture 决策（starter template + 实施模式）分解为可实施的 Story。

> **范围聚焦：MVP 阶段（49 项 MVP FR + 38 项 MVP NFR + UX/Architecture 附加需求）**。Growth 与 Vision 范围的 FR 仅作引用，不纳入本 Epic 列表。

## Requirements Inventory

### Functional Requirements

> 来自 PRD §9（共 63 项；本节列出全部，MVP/Growth/Vision 标签如 PRD 定义）

**A. 部署与初始化（Deployment & Initialization）**

- **FR1** [MVP] 任何人可以通过单条容器运行命令完成服务端部署（不依赖外部数据库/缓存）
- **FR2** [MVP] 系统能在首次启动时自动申请并配置 HTTPS 证书（基于域名）
- **FR3** [MVP] 系统能在首次访问 Web 后台时引导首位访问者创建初始 admin 账号
- **FR4** [MVP] admin 可以在后台查看服务端配置信息（VPN 网段、服务端公钥、监听端点、版本号）
- **FR5** [MVP] 系统在重启后能自动从持久化存储恢复所有节点的 WireGuard 配置，无需 admin 介入

**B. 账号与认证（Account & Authentication）**

- **FR6** [MVP] 用户可以用账号+密码登录系统（通过 CLI 或 Web 后台）
- **FR7** [MVP] 用户可以修改自己的密码
- **FR8** [MVP] 用户首次登录使用初始密码时，系统强制要求修改为自定义密码后才能继续操作
- **FR9** [MVP] 已登录用户可以主动注销当前会话
- **FR10** [MVP] 系统能在用户连续多次登录失败后临时锁定该账号
- **FR11** [MVP] 系统会在登录失败时返回统一的错误信息（不区分用户名错误与密码错误）
- **FR12** [MVP] 系统能区分 admin 与普通 user 两种角色，并对应不同的可访问操作
- **FR13** [G] 系统支持 RBAC 三角色（Admin / Operator / User），可配置更细粒度的权限

**C. 用户管理（User Management - admin）**

- **FR14** [MVP] admin 可以创建新用户（指定用户名、邮箱，由系统自动生成强初始密码）
- **FR15** [MVP] admin 可以查看所有用户列表（含状态、创建时间、最后登录时间）
- **FR16** [MVP] admin 可以分页与按用户名/邮箱搜索用户列表
- **FR17** [MVP] admin 可以重置任意用户的密码（重置后用户下次登录需强制改密）
- **FR18** [MVP] admin 可以禁用/启用用户账号（禁用账号无法登录但保留数据）
- **FR19** [MVP] admin 可以删除用户账号（级联清理其所有节点配置与活跃 Token）
- **FR20** [MVP] admin 可以复制系统生成的"员工接入指南"链接（包含客户端下载、登录步骤）

**D. 节点（Peer）管理与连接**

- **FR21** [MVP] 已登录用户可以从其客户端注册一个新节点（提交本地生成的 WireGuard 公钥）
- **FR22** [MVP] 系统能为新注册的节点自动从 VPN 网段中分配唯一虚拟 IP
- **FR23** [MVP] 系统能为已注册节点保持稳定的虚拟 IP 绑定（重连/重启不更换 IP）
- **FR24** [MVP] 注册成功的节点可以下载其完整 WireGuard 配置（含服务端公钥、虚拟 IP、网段、端点）
- **FR25** [MVP] 已连接节点可以定期发送心跳，系统据此更新节点的在线状态与公网 endpoint
- **FR26** [MVP] 用户可以主动注销自己的节点（断开隧道并从服务端移除 peer）
- **FR27** [MVP] admin 可以查看所有节点列表（含所属用户、虚拟 IP、在线状态、最后心跳时间、公网 endpoint、操作系统）
- **FR28** [MVP] admin 可以强制下线任意节点（立即从服务端 WireGuard 配置移除）
- **FR29** [MVP] 系统能在节点连续未发送心跳超过阈值后自动标记为离线状态

**E. 客户端体验（Client Experience）**

- **FR30** [MVP] 用户可以从设置引导页下载对应操作系统的客户端安装包（自动识别 OS）
- **FR31** [MVP] 客户端提供安装包形式安装（macOS .pkg / Windows .msi / Linux .deb/.rpm/tar.gz）
- **FR32** [MVP] 用户可以通过 CLI 命令登录服务端（交互式输入密码）
- **FR33** [MVP] 用户可以通过 CLI 命令启动 / 停止 VPN 隧道
- **FR34** [MVP] 用户可以通过 CLI 命令查看当前连接状态（虚拟 IP、对端、流量、在线时长）
- **FR35** [MVP] 客户端能将凭证（Refresh Token）安全存储到操作系统的安全凭据库（Keychain / libsecret / Credential Manager）
- **FR36** [MVP] 客户端能将 WireGuard 私钥本地加密存储，不上传到服务端
- **FR37** [MVP] 客户端能以后台 daemon 方式常驻运行（systemd / launchd / Windows Service）
- **FR38** [MVP] 所有 CLI 命令支持 `--json` 参数以结构化格式输出（便于脚本化）
- **FR39** [G] 用户可以通过桌面 GUI 客户端（替代 CLI）完成连接、状态查看
- **FR40** [V] 用户可以从 iOS / Android 应用接入 VPN

**F. 隧道运维（Tunnel Operations）**

- **FR41** [MVP] 系统能在客户端与服务端之间建立加密 VPN 隧道（基于 WireGuard 协议）
- **FR42** [MVP] 已连接节点间可以通过虚拟 IP 互相通信（经服务端 Hub-Spoke 转发）
- **FR43** [MVP] 客户端能在网络断开或服务端不可达时自动尝试重连（指数退避）
- **FR44** [MVP] 客户端能在本机网络环境变化时（Wi-Fi 切换等）自动检测并重建隧道
- **FR45** [MVP] 客户端能向用户显示当前连接状态变化（连接中 / 已连接 / 已断开）
- **FR46** [V] 节点之间可以通过 P2P 直连方式通信（绕过服务端转发）
- **FR47** [V] 客户端能在双重 NAT/CGNAT 环境下与对端建立 P2P 直连（UDP 打洞）

**G. 审计与可见性（Audit & Visibility）**

- **FR48** [MVP] 系统能记录所有登录尝试（含用户名、IP、UA、成功/失败、失败原因）
- **FR49** [MVP] 系统能记录所有配置变更操作（用户增删、密码重置、节点强制下线等）
- **FR50** [MVP] 系统能记录所有节点连接/断开事件
- **FR51** [MVP] admin 可以在后台查询审计日志（含分页、按时间/用户/操作类型过滤）
- **FR52** [MVP] 系统能将审计日志保留至少 6 个月
- **FR53** [G] admin 可以查看每节点 / 每用户的流量统计（入站/出站字节数、时间段聚合）
- **FR54** [G] admin 可以通过 WebSocket 实时接收节点状态变化推送（无需轮询）
- **FR55** [G] 系统能以 Prometheus metrics 格式暴露关键指标（在线节点数、流量、认证失败率）

**H. 安全防护（Security Protection）**

- **FR56** [MVP] 系统能对登录端点做请求限速（防爆破）
- **FR57** [MVP] 系统能在认证失败超过阈值后临时锁定账号（指数退避）
- **FR58** [MVP] 系统能使用 argon2id 算法存储用户密码
- **FR59** [MVP] 系统能强制使用 HTTPS 访问管理后台（拒绝 HTTP 请求）
- **FR60** [MVP] 系统能在用户被删除时原子清理其所有相关数据（peer 配置 + Token + 凭据）
- **FR61** [MVP] 系统能验证创建/修改密码时的密码强度（最小长度、字符种类）
- **FR62** [G] admin 可以配置管理后台访问的 IP 白名单（限制后台访问来源）
- **FR63** [V] admin 可以将系统接入企业 IDP（LDAP / OAuth2 / SAML）进行单点登录

**统计：MVP 49 项 / Growth 8 项 / Vision 6 项**

### NonFunctional Requirements

> 来自 PRD §10（共 43 项，MVP 38 / G 5）

**Performance：**

- **NFR-P1** [MVP] 单 VPN 隧道在千兆带宽下吞吐 ≥ 100 Mbps（boringtun userspace 实测）
- **NFR-P2** [MVP] 50 个并发节点场景下，服务端 CPU 占用 < 30%（4 vCPU 主机）
- **NFR-P3** [MVP] 50 个并发节点场景下，服务端常驻内存 < 1 GB
- **NFR-P4** [MVP] 客户端从 `vpn-cli start` 到隧道建立完成 ≤ 10 秒
- **NFR-P5** [MVP] 管理后台 API（用户列表、节点列表）p95 响应时间 ≤ 300 ms（50 节点数据量）
- **NFR-P6** [MVP] 客户端心跳间隔 = 30 秒；WireGuard PersistentKeepalive = 25 秒
- **NFR-P7** [G] 单 VPN 隧道吞吐 ≥ 500 Mbps（性能优化目标）

**Security：**

- **NFR-S1** [MVP] 用户密码使用 argon2id 哈希存储（参数 m=64MB, t=3, p=2）
- **NFR-S2** [MVP] 所有管理后台流量强制 HTTPS（TLS 1.2+），禁用 SSLv3/TLS 1.0/1.1
- **NFR-S3** [MVP] VPN 隧道使用 WireGuard 默认套件（Curve25519 + ChaCha20-Poly1305 + BLAKE2s）
- **NFR-S4** [MVP] JWT 使用 RS256 非对称签名；Access Token 有效期 ≤ 15 分钟
- **NFR-S5** [MVP] 登录失败 5 次后账号锁定 ≥ 15 分钟（指数退避）
- **NFR-S6** [MVP] 登录端点限速 ≤ 5 次/分钟/IP
- **NFR-S7** [MVP] 客户端 WireGuard 私钥仅在客户端本地生成与存储，服务端永不接触
- **NFR-S8** [MVP] 服务端二进制以非 root 用户运行，仅授予 CAP_NET_ADMIN capability
- **NFR-S9** [MVP] 密码与 Token 在日志中必须脱敏（不输出原始值）
- **NFR-S10** [MVP] 审计日志保留 ≥ 6 个月（合规要求）
- **NFR-S11** [MVP] 密码最小强度：≥ 8 位，至少包含字母 + 数字

**Reliability：**

- **NFR-R1** [MVP] 服务端连续运行 ≥ 30 天不需重启（验证内存泄漏与稳定性）
- **NFR-R2** [MVP] 月度可用性 ≥ 99.5%（每月不可用时间 ≤ 3.6 小时）
- **NFR-R3** [MVP] 客户端断线后自动重连成功率 ≥ 99%（网络抖动、服务端重启场景）
- **NFR-R4** [MVP] 服务端进程异常退出后能由 systemd/Docker 自动重启
- **NFR-R5** [MVP] 服务端重启后能在 30 秒内从持久化存储恢复所有 WireGuard peer 配置
- **NFR-R6** [MVP] 数据库写入失败时不阻塞客户端心跳响应（降级写本地缓存，后台重试）
- **NFR-R7** [MVP] WireGuard timer 必须每 ≤ 250ms 调用一次 `update_timers()`（防静默断线）

**Scalability：**

- **NFR-SC1** [MVP] 单服务端实例稳定支持 ≥ 50 并发活跃节点
- **NFR-SC2** [MVP] 单服务端实例稳定支持 ≥ 500 注册用户账号（离线节点占多数）
- **NFR-SC3** [G] 单服务端实例支持 ≥ 200 并发活跃节点（性能优化目标）
- **NFR-SC4** [G] 数据库存储层可平滑从 SQLite 切换到 PostgreSQL（无业务逻辑变更）

**Compatibility：**

- **NFR-C1** [MVP] 服务端在 Linux x86_64 内核 ≥ 4.19 上运行（推荐 ≥ 5.6）
- **NFR-C2** [MVP] 客户端在 Linux x86_64/aarch64（Ubuntu 20.04+/Debian 11+/CentOS 8+）运行
- **NFR-C3** [MVP] 客户端在 macOS ≥ 11.0（Big Sur）x86_64/aarch64 运行
- **NFR-C4** [MVP] 客户端在 Windows ≥ 10 (1809) x86_64 运行
- **NFR-C5** [MVP] 三平台客户端功能完全对等（同样 CLI 命令、行为）
- **NFR-C6** [MVP] 管理后台前端兼容 Chrome/Edge/Safari/Firefox 最近 2 个主版本

**Operability：**

- **NFR-O1** [MVP] 服务端从首次容器启动到管理后台可访问 ≤ 5 分钟（含自动 HTTPS 证书申请）
- **NFR-O2** [MVP] 一次完整部署所需手动操作 ≤ 3 步（拉取镜像 / 启动容器 / 首次访问 setup）
- **NFR-O3** [MVP] 部署完成后无需 SSH 进入服务器即可完成全部日常运维（账号、节点、日志）
- **NFR-O4** [MVP] 客户端安装到首次连接 ≤ 3 分钟（按引导操作）
- **NFR-O5** [MVP] 服务端启动失败时通过 stderr 输出明确错误（数据库不可达 / 端口被占 / 证书失败）
- **NFR-O6** [MVP] 服务端配置变更（添加用户等）即时生效，无需重启服务
- **NFR-O7** [MVP] README 文档完整覆盖部署、使用、故障排查（中文，附截图）
- **NFR-O8** [G] 服务端能在 systemd-journald 输出结构化 JSON 日志（便于采集分析）

**Maintainability：**

- **NFR-M1** [MVP] 核心业务模块单元测试覆盖率 ≥ 70%（认证、Peer 管理、WireGuard 集成）
- **NFR-M2** [MVP] 提供至少 3 个端到端集成测试覆盖核心用户旅程（部署、新员工接入、断线重连）
- **NFR-M3** [MVP] CI 在每次 PR 提交时运行 `cargo fmt --check` + `cargo clippy -D warnings` + `cargo test`
- **NFR-M4** [MVP] CI 矩阵测试 Linux + macOS + Windows 三平台编译通过
- **NFR-M5** [MVP] 所有公开 API 端点提供 OpenAPI 文档（Markdown 形式）

### Additional Requirements

> 来自 Architecture（含 starter template、基础设施、跨切关注点）

**Starter / 项目初始化（Epic 1, Story 1 必含）：**

- AR-S1 项目使用手动 `cargo new --workspace` 初始化 Rust workspace（不使用任何重型 starter）
- AR-S2 前端使用 Vite `npm create vite@latest frontend -- --template react-ts` 初始化
- AR-S3 Workspace 含 6 个子 crate：`vpn-api-types`、`vpn-core`、`vpn-wireguard`、`vpn-platform`、`vpn-server`、`vpn-cli`
- AR-S4 Rust 工具链锁定到 stable ≥ 1.82（rust-toolchain.toml）
- AR-S5 项目根添加 Justfile 作为统一开发命令入口

**基础设施与部署：**

- AR-D1 Docker 单容器部署（Dockerfile + cargo-chef 多阶段构建）
- AR-D2 docker-compose.yml（生产部署）+ docker-compose.test.yml（E2E 测试）
- AR-D3 客户端安装包构建脚本：`installers/{macos,windows,linux}/`
- AR-D4 GitHub Actions CI：三平台（Linux/macOS/Windows）矩阵 + Rust 缓存
- AR-D5 GitHub Actions Release：tag 触发 + 构建二进制 + 推送 Docker 镜像

**集成与外部依赖：**

- AR-I1 ACME 自动证书申请（`rustls-acme` 集成 Let's Encrypt）
- AR-I2 客户端 daemon ↔ CLI 通过 Unix Socket / Named Pipe + JSON-RPC 2.0
- AR-I3 客户端凭证存储：macOS Keychain / Linux libsecret / Windows Credential Manager
- AR-I4 前端通过 `rust-embed` 嵌入服务端二进制（单容器部署）

**数据与持久化：**

- AR-DB1 SQLite + sqlx；migrations 在 `migrations/` 目录（命名 `YYYYMMDDHHMMSS_<name>.sql`）
- AR-DB2 sqlx 离线模式（`SQLX_OFFLINE=true` + 提交 `.sqlx/`）以支持无数据库 CI 构建
- AR-DB3 Repository Pattern 抽象（便于后期切 PostgreSQL）

**横切关注点（Cross-Cutting）：**

- AR-X1 错误处理：`thiserror::AppError` + `IntoResponse` + 统一 ApiResponse 信封
- AR-X2 日志：`tracing` + JSON 格式输出到 stdout
- AR-X3 请求 ID：tower-http 中间件自动注入 `X-Request-Id`
- AR-X4 限速：`tower-governor`（GCRA 算法）
- AR-X5 优雅关闭：SIGTERM → 等待 in-flight → 关闭 WireGuard → 退出
- AR-X6 启动校验：数据库连接 + WireGuard 接口创建 + TLS 证书 + 明确错误
- AR-X7 实施模式与一致性规则：35+ 命名/结构/格式/通信/流程规则 由 CI 强制（rustfmt + clippy -D warnings + ESLint + tsc）

### UX Design Requirements

> 来自 UX Spec §5/§7/§8/§10/§11/§12（共 22 项 UX-DR）

**设计系统与视觉基础：**

- **UX-DR1** [MVP] 配置 AntD 5 ConfigProvider + theme tokens（geekblue primary `#2F54EB`、4 状态色、深色侧边栏 `#001529`、圆角 4px）
- **UX-DR2** [MVP] 配置系统字体优先策略（不加载 Web Font；PingFang SC / Microsoft YaHei）
- **UX-DR3** [MVP] 配置等宽字体（CLI 命令、虚拟 IP 显示用 SF Mono / Cascadia Code 等）
- **UX-DR4** [MVP] 配置 4 档响应式断点（xs/md/lg/xl）+ 主内容区 max-width 1600px 限宽

**自建组件（UX Spec §10）：**

- **UX-DR5** [MVP] 实现 `<NodeStatusDot>` 组件（4 状态色点 + 文字 + tooltip + 3 种尺寸）
- **UX-DR6** [MVP] 实现 `<SetupWizard>` 3 步引导（创建 admin / 配置网段 / 添加首用户 + 成功卡片）
- **UX-DR7** [MVP] 实现 `<CopyLink>` 一键复制组件（inline/block/multi 模式 + 即时反馈）
- **UX-DR8** [MVP] 实现 `<ConnectionGuide>` 员工接入引导页组件（OS 检测 + 平台切换）
- **UX-DR9** [MVP] 实现 `<EmptyStateWithAction>` 引导式空状态（4 种 variant）

**UI 页面（UX Spec §8）：**

- **UX-DR10** [MVP] 实现 ProLayout 主框架（深色侧边栏 4 项 + 顶栏用户菜单）
- **UX-DR11** [MVP] 实现 Setup Wizard 首次部署引导页（公开路由 + 仅首次访问可用）
- **UX-DR12** [MVP] 实现登录页（账号密码 + 首次改密提示）
- **UX-DR13** [MVP] 实现仪表盘首页（4 KPI 卡片 + 最近节点列表 + 待办告警）
- **UX-DR14** [MVP] 实现用户管理页（ProTable + 搜索筛选 + CRUD + 重置密码 + ShareLinkModal）
- **UX-DR15** [MVP] 实现节点管理页（ProTable + 状态色点 + 强制下线）
- **UX-DR16** [MVP] 实现审计日志页（ProTable + 时间/用户/类型筛选）
- **UX-DR17** [MVP] 实现员工接入引导页 `/setup`（公开 + 自动检测 OS + 下载 + CLI 命令复制）
- **UX-DR18** [MVP] 实现账号设置页（修改密码）

**一致性模式（UX Spec §11/§12）：**

- **UX-DR19** [MVP] 实现统一反馈模式（message / notification / Alert / Modal.confirm 四类使用决策）
- **UX-DR20** [MVP] 实现错误即操作（每个错误提示附"建议操作"按钮）
- **UX-DR21** [MVP] 实现 axios 拦截器（snake_case ↔ camelCase 转换 + 统一 ApiResponse 解包）
- **UX-DR22** [MVP] 实现 useWebSocket Hook + nodeStore（节点实时状态推送 + Zustand 状态管理）

### FR Coverage Map

| FR | Epic | 说明 |
|----|------|------|
| FR1 容器部署 | Epic 1 | Dockerfile + entrypoint |
| FR2 ACME HTTPS | Epic 1 | rustls-acme 集成 |
| FR3 首次设置引导 | Epic 2 | SetupWizard 第 1 步 |
| FR4 查看服务端配置 | Epic 2 | 仪表盘配置卡片 |
| FR5 重启恢复 WG 配置 | Epic 4 | wireguard_runtime/reload_all_peers |
| FR6-12 认证（7 项） | Epic 2 | auth_service + middleware/auth |
| FR14-20 用户 CRUD（7 项） | Epic 3 | user_service + UserList page |
| FR21-26 节点自服务（6 项） | Epic 4 | peer_service + vpn-cli |
| FR27-29 admin 节点监控（3 项） | Epic 5 | admin_peers + 节点列表页 |
| FR30-38 客户端体验（9 项） | Epic 4 | vpn-cli + vpn-platform |
| FR41-45 隧道运维（5 项） | Epic 4 | boringtun + reconnect |
| FR48-52 审计（5 项） | Epic 5 | audit_service + AuditLog page |
| FR56 限速 | Epic 2 | tower-governor on /auth/login |
| FR57 账号锁定 | Epic 2 | ratelimit/login.rs |
| FR58 argon2id | Epic 2 | password_hasher |
| FR59 强制 HTTPS | Epic 1 | middleware/https_only |
| FR60 级联清理 | Epic 3 | user_service::delete (事务) |
| FR61 密码强度 | Epic 2 | validator on CreateUserRequest |

**FR 覆盖率：49 项 MVP FR 100% 映射到 Epic（无遗漏）**

| Growth/Vision FR | 状态 |
|---|---|
| FR13, FR39, FR53-55, FR62, FR63 | Growth — 不在 MVP Epic |
| FR40, FR46-47 | Vision — 不在 MVP Epic |

## Epic List

### Epic 1: Project Foundation & Deployable Skeleton（项目基础与可部署骨架）

**Epic Goal：** 让任何开发者能 `git clone → docker run → 浏览器访问域名 → 看到"项目正在运行"占位页`，验证"5 分钟部署"承诺的基础设施层。

**User Outcome：**

- 开发者：能开始所有后续 Epic 的开发工作（workspace 就绪、CI 跑通、前端骨架可启动）
- 终端用户（IT 管理员）：能在 5 分钟内启动服务并看到 HTTPS 页面（虽然还没有登录功能）

**FRs covered：** FR1（容器部署）, FR2（自动 HTTPS）, FR59（强制 HTTPS）

**NFRs covered：** NFR-O1, NFR-O2, NFR-S2, NFR-S8, NFR-S9（日志脱敏框架）, NFR-M3, NFR-M4

**AR covered：** AR-S1~S5（Workspace 初始化）, AR-D1~D5（Docker + CI/CD）, AR-I1（ACME）, AR-I4（rust-embed）, AR-DB1~DB2（migrations + sqlx 离线）, AR-X1~X6（错误处理、日志、请求 ID、限速、优雅关闭、启动校验）, AR-X7（一致性规则 CI 配置）

**UX-DR covered：** UX-DR1~DR4（设计系统配置 + 字体 + 断点）, UX-DR10（ProLayout 主框架）

---

### Epic 2: Admin Authentication & First-Run Setup（Admin 认证与首次设置）

**Epic Goal：** 让 admin 能完成首次访问的引导式设置（创建 admin 账号 + 配置 VPN 网段 + 创建首个用户）+ 后续登录、改密、注销操作。

**User Outcome：**

- IT 管理员：能完成"首次部署"旅程（J1.1），看到完整的 setup wizard 引导流程
- 完成 Epic 2 后：admin 能登录看到（空的）仪表盘，并查看服务端配置信息

**FRs covered：** FR3（首次设置引导）, FR4（查看服务端配置）, FR6-12（认证 7 项）, FR56（限速）, FR57（账号锁定）, FR58（argon2id）, FR61（密码强度）

**NFRs covered：** NFR-S1, NFR-S4, NFR-S5, NFR-S6, NFR-S11, NFR-S9

**UX-DR covered：** UX-DR6（SetupWizard）, UX-DR12（登录页）, UX-DR13（仪表盘首页）, UX-DR18（账号设置页）, UX-DR19~DR21（反馈/错误/axios 拦截器）

**关键依赖：** 依赖 Epic 1 的项目骨架（无新增依赖项）

---

### Epic 3: User Account Management（用户账号管理）

**Epic Goal：** 让 admin 能完整管理用户账号（CRUD + 重置密码 + 禁用 + 删除 + 复制接入链接）。

**User Outcome：**

- IT 管理员：能完成"添加新员工"旅程（J1.2），3 分钟内创建用户并生成接入链接
- 完成 Epic 3 后：管理后台用户管理功能完整可用，但用户还无法用客户端连接（需要 Epic 4）

**FRs covered：** FR14-20（用户 CRUD 7 项）, FR60（级联清理）

**NFRs covered：** NFR-P5（API 响应 ≤ 300ms）

**UX-DR covered：** UX-DR7（CopyLink）, UX-DR9（EmptyStateWithAction）, UX-DR14（用户管理页 + CreateUserModal + ShareLinkModal）

**关键依赖：** 依赖 Epic 2 的认证子系统（JWT + admin 角色判定）

---

### Epic 4: VPN Tunnel & Client Connectivity（VPN 隧道与客户端连接）

**Epic Goal：** 实现 VPN 数据平面 + 跨平台客户端，让用户能从 macOS/Linux/Windows 通过账号密码登录 + 自动连接 VPN + 节点间互通。

**User Outcome：**

- 普通员工：能完成"首次接入"旅程（J2.1）+ "断线恢复"旅程（J2.2）
- IT 管理员：服务端能持久化 peer 配置，重启自动恢复
- 完成 Epic 4 后：核心 VPN 功能全部可用，可以真实做异地组网

**FRs covered：** FR5（重启恢复）, FR21-26（节点自服务 6 项）, FR30-38（客户端体验 9 项）, FR41-45（隧道运维 5 项）

**NFRs covered：** NFR-P1~P4, NFR-P6, NFR-S3（WireGuard 加密）, NFR-S7（私钥隔离）, NFR-R1, NFR-R3, NFR-R5, NFR-R6, NFR-R7（WG timer 100ms 关键陷阱）, NFR-C1~C5（跨平台对等）

**AR covered：** AR-I2（IPC）, AR-I3（凭证存储）

**UX-DR covered：** UX-DR8（ConnectionGuide）, UX-DR17（员工接入引导页）

**关键依赖：** 依赖 Epic 2（认证）+ Epic 3（用户存在）

---

### Epic 5: Peer Monitoring & Audit Visibility（节点监控与审计可见性）

**Epic Goal：** 让 admin 能在后台可视化监控所有节点状态 + 查询审计日志 + 独立完成故障排查（不需 SSH）。

**User Outcome：**

- IT 管理员：能完成"故障排查"旅程（J1.3），30 秒内定位问题
- 完成 Epic 5 后：管理后台运维体验完整，admin 可独立处理日常运维

**FRs covered：** FR27（admin 查看节点列表）, FR28（强制下线）, FR29（自动离线检测）, FR48-52（审计 5 项）

**NFRs covered：** NFR-S10（日志保留 6 月）

**UX-DR covered：** UX-DR5（NodeStatusDot 完整使用）, UX-DR15（节点管理页）, UX-DR16（审计日志页）, UX-DR22（WebSocket 实时状态推送 - 仅 admin 维度，标 Growth 范围）

> 注：FR54（WebSocket admin 事件推送）是 Growth 范围，本 Epic 仅做静态轮询（10 秒）；WebSocket 留到 Phase 2。
> 注：FR27/28/29 涉及节点列表 API（peer-related），与 Epic 4 的 peer 注册/心跳 API 同模块，但通过子模块拆分（`handlers/admin_peers.rs` vs `handlers/peers.rs`），文件冲突最小化。

**关键依赖：** 依赖 Epic 4（peer 数据已有）

---

### Epic 6: Production Release（生产发布准备）

**Epic Goal：** 让项目能正式发布到 GitHub Releases + Docker Hub，含完整文档、E2E 测试、客户端安装包。

**User Outcome：**

- 项目维护者：能发布 v0.1.0 给真实用户使用
- 用户：能从 README quickstart 顺利部署 + 找到故障排查文档 + 从 GitHub Releases 下载客户端安装包

**FRs covered：** 无新 FR（验证类）

**NFRs covered：** NFR-R2（可用性 99.5%）, NFR-R4（自动重启）, NFR-O3~O7（运维文档）, NFR-C6（浏览器兼容），NFR-M1（70% 测试覆盖率达标）, NFR-M2（3 个 E2E 测试达标）, NFR-M5（API 文档）

**AR covered：** AR-D3（客户端安装包构建）, AR-D5（Release CI）

**关键依赖：** 依赖 Epic 1-5 全部完成

---

## Epic 依赖图

```
Epic 1（基础设施）
    │
    ▼
Epic 2（认证 + Setup）
    │
    ├──────────────┐
    ▼              ▼
Epic 3       Epic 4（VPN 隧道）
（用户管理）        │
    │              │
    └──────┬───────┘
           ▼
        Epic 5（监控与审计）
           │
           ▼
        Epic 6（生产发布）
```

注：Epic 3 与 Epic 4 在 Epic 2 完成后可并行（同人项目则按顺序）。

---

## Epic 1: Project Foundation & Deployable Skeleton

让任何开发者能 `git clone → docker run → 浏览器访问域名 → 看到"项目正在运行"占位页`，验证"5 分钟部署"承诺的基础设施层。

### Story 1.1: 初始化 Rust workspace 与前端项目

As a 项目开发者,
I want 一个标准化的 Rust workspace + Vite React-TS 前端骨架,
So that 后续所有 Epic 都有清晰的代码组织起点.

**Acceptance Criteria:**

**Given** 干净的工作目录
**When** 按照架构文档 §3 执行初始化命令（`cargo new --workspace` + 6 个子 crate + `npm create vite`）
**Then** 项目根包含 `Cargo.toml`（workspace 配置含 `[workspace.dependencies]`）、`rust-toolchain.toml`（stable 1.82+）、`Justfile`、`.gitignore`、`.editorconfig`、`README.md` 占位
**And** `crates/` 下存在 6 个子 crate（vpn-api-types/vpn-core/vpn-wireguard/vpn-platform/vpn-server/vpn-cli）
**And** `frontend/` 下存在 Vite + React 18 + TypeScript strict 项目骨架

**Given** 已完成项目初始化
**When** 执行 `cargo build --workspace` 与 `cd frontend && npm install && npm run build`
**Then** 后端编译成功（仅占位 hello world）；前端构建成功（生成 `frontend/dist/`）
**And** `just --list` 显示所有可用开发命令（dev / test / build / docker）

### Story 1.2: 配置 sqlx 与初始数据库 migration

As a 项目开发者,
I want sqlx + SQLite 集成并支持离线编译,
So that CI 无需数据库连接即可编译 + 后续 Epic 能直接添加 migration.

**Acceptance Criteria:**

**Given** vpn-server crate 已存在
**When** 配置 sqlx + SQLite feature + 添加 `migrations/` 目录与初始空 migration（`20260511_120000_init.sql`）
**Then** `sqlx-cli` 可执行 `sqlx migrate run` 创建空数据库
**And** `cargo sqlx prepare` 生成 `.sqlx/` 离线缓存目录

**Given** 已生成 `.sqlx/` 缓存且已提交到 git
**When** 在无 `DATABASE_URL` 环境变量场景下设置 `SQLX_OFFLINE=true` 后执行 `cargo build`
**Then** 编译成功不报错
**And** CI 配置 `SQLX_OFFLINE=true` 后无需数据库依赖

### Story 1.3: 实现 vpn-api-types 共享 DTO 基础

As a 前后端开发者,
I want 一个独立、零 IO 依赖的 DTO crate,
So that 前后端类型契约统一，后端 DTO 修改即时反映到前端类型.

**Acceptance Criteria:**

**Given** vpn-api-types crate 已存在
**When** 实现 `ApiResponse<T>` 信封（含 code/message/data/timestamp/request_id）+ `Page<T>` 分页结构 + `error_codes` 常量模块
**Then** crate 编译通过，依赖仅含 `serde` 与 `serde_json`（无 sqlx/axum/reqwest）
**And** 提供单元测试覆盖成功/错误响应序列化与反序列化

### Story 1.4: 实现 vpn-core 错误处理与 trait 基础

As a 项目开发者,
I want 一组统一的领域错误类型与无具体实现的 trait,
So that 业务层与 IO 层解耦，便于测试和后期切换实现.

**Acceptance Criteria:**

**Given** vpn-core crate 已存在
**When** 定义 `AppError` enum（用 thiserror，含 InvalidCredentials/TokenExpired/UserNotFound/Database/Internal 等业务错误）+ Repository trait 占位（UserRepository/PeerRepository/SessionRepository/AuditRepository）+ PasswordHasher/TokenIssuer/IdGenerator/Clock trait
**Then** crate 仅依赖 vpn-api-types 与 thiserror、anyhow、async-trait，无 sqlx/axum
**And** AppError 提供单元测试映射到 ApiResponse 错误码

### Story 1.5: 实现 vpn-server 启动骨架与中间件链

As a IT 管理员（开发期）,
I want 一个能 `cargo run` 启动并响应 /health 的最小服务端,
So that 后续 Epic 能基于此添加业务功能.

**Acceptance Criteria:**

**Given** vpn-server crate 与 tokio + axum 依赖已配置
**When** 实现 main.rs（< 50 行）、app.rs（Axum Router 工厂）、state.rs（AppState）、startup.rs（启动校验）、shutdown.rs（SIGTERM 优雅关闭）、middleware/{request_id, error}.rs
**Then** `cargo run --bin vpn-server` 启动监听 `0.0.0.0:8080`，访问 `http://localhost:8080/health` 返回 200 + JSON `{"code":0,"data":"ok"}`
**And** 每个响应都包含 `X-Request-Id` Header
**And** 配置 `tracing-subscriber` JSON 格式输出到 stdout（日志级别由 `RUST_LOG` 控制）

**Given** 服务已启动
**When** 发送 SIGTERM 信号
**Then** 服务停止接收新请求，等待 in-flight 请求完成（最长 30s）后优雅退出（exit 0）

### Story 1.6: 实现 ACME 自动 HTTPS 与强制 HTTPS 中间件

As a IT 管理员,
I want 服务端首次启动自动申请并配置 HTTPS 证书（无需手动操作）,
So that 部署后立即可通过 HTTPS 访问，符合 PRD §6 FR2 与"5 分钟部署"承诺.

**Acceptance Criteria:**

**Given** 服务端已配置域名（通过 `VPN_DOMAIN` 环境变量）
**When** 首次启动（无现有证书）
**Then** 系统通过 rustls-acme 自动从 Let's Encrypt 申请证书并保存到本地磁盘
**And** HTTPS 监听 443 端口（或 `VPN_HTTPS_PORT` 配置）
**And** HTTP 监听 80 端口仅处理 ACME challenge + 其他请求 301 重定向到 HTTPS

**Given** 域名解析失败或网络不可达
**When** 启动 ACME 流程
**Then** 服务端通过 stderr 输出明确错误（`ACME 证书申请失败：域名 X 未解析到本机`）并退出 exit 1（不静默挂起）

**Given** HTTPS 已就绪
**When** 任何 HTTP 请求到达
**Then** 中间件返回 301 跳转到对应 HTTPS URL（除 ACME challenge 路径外）

### Story 1.7: 实现 React 前端骨架与 AntD 主题配置

As a 前端开发者,
I want 一个配置好 AntD 主题、状态管理、HTTP 拦截器的 React 骨架,
So that 后续 Epic 能直接添加页面而无需重复配置.

**Acceptance Criteria:**

**Given** frontend 项目骨架已初始化
**When** 安装 antd 5 + @ant-design/pro-components + @tanstack/react-query + zustand + axios + react-router-dom + dayjs
**And** 配置 `theme.ts`（geekblue primary `#2F54EB`、4 状态色、深色侧边栏 `#001529`、圆角 4px、系统字体）
**And** 配置 ConfigProvider 全局 zh-CN locale + theme
**And** 配置 axios 拦截器（请求头注入 Authorization、响应自动 snake_case → camelCase 转换、ApiResponse 自动解包、统一错误 toast）
**And** 配置 React Query QueryClient + Zustand store 占位
**Then** `npm run dev` 启动后访问 `localhost:5173` 看到应用 AntD 主题的"vpn 项目正在运行"占位页

**Given** ProLayout 主框架已实现
**When** 渲染 AppLayout
**Then** 显示深色侧边栏（4 项导航占位：仪表盘/用户/节点/日志） + 顶栏（项目名 + 用户菜单占位）

### Story 1.8: 实现 rust-embed 嵌入前端 + SPA fallback

As a IT 管理员,
I want 服务端单二进制即包含前端静态资源,
So that 部署仅需一个 docker run 命令（无需额外 nginx 或前端服务）.

**Acceptance Criteria:**

**Given** frontend/dist 已构建
**When** vpn-server 添加 `build.rs` 调用 rust-embed 嵌入 `frontend/dist/`
**Then** `cargo build --release` 产物体积包含前端资源（约 +1-2MB）

**Given** 服务端已启动
**When** 浏览器请求 `GET /` 或任何非 `/api/*` 与非 `/ws/*` 的 GET 路径
**Then** 返回 `index.html`（SPA fallback），允许 React Router 处理前端路由
**And** 请求 `GET /assets/index-*.js` 返回对应静态资源（含正确 Content-Type 与缓存头）

### Story 1.9: 实现 Docker 多阶段构建 + Dockerfile

As a IT 管理员,
I want 一个 docker run 命令完成服务端部署,
So that 满足 PRD NFR-O1（5 分钟部署）与 NFR-O2（≤ 3 步操作）.

**Acceptance Criteria:**

**Given** Cargo.toml 与 frontend 已就绪
**When** 创建 `docker/Dockerfile`（多阶段：planner + cargo-chef + builder + frontend builder + runtime）
**And** 创建 `docker/docker-compose.yml`（含 volume 挂载 `/var/lib/vpn`、端口映射 80/443、环境变量 `VPN_DOMAIN`）
**And** 创建 `docker/entrypoint.sh`（启动前 sqlx migrate 后启动 vpn-server）
**Then** `docker build -f docker/Dockerfile -t vpn:latest .` 构建成功，最终镜像 < 150MB
**And** 镜像以非 root 用户 `vpn:vpn` 运行（带 CAP_NET_ADMIN capability）

**Given** Docker 镜像已构建
**When** 执行 `docker run -p 80:80 -p 443:443 -e VPN_DOMAIN=vpn.example.com -v vpn-data:/var/lib/vpn vpn:latest`
**Then** 容器启动成功
**And** 5 分钟内访问 `https://vpn.example.com/health` 返回 200

### Story 1.10: 配置 GitHub Actions CI 三平台矩阵

As a 项目维护者,
I want CI 在每个 PR 自动运行格式/lint/测试/三平台编译,
So that 不一致的代码无法合入主分支.

**Acceptance Criteria:**

**Given** 项目已推送到 GitHub
**When** 创建 `.github/workflows/ci.yml`（含 `Swatinem/rust-cache@v2`、`SQLX_OFFLINE=true`）
**Then** push 或 PR 触发 CI 流程，并行运行：
  - Rust: `cargo fmt --all -- --check` + `cargo clippy --all-targets -- -D warnings` + `cargo test --workspace`
  - Frontend: `npm run lint` + `npm run typecheck` + `npm run build`
  - 三平台矩阵：ubuntu-latest / macos-latest / windows-latest 均编译通过
**And** 任何步骤失败 CI 红灯，阻止合入

**Given** CI 已配置
**When** 提交不符合 `cargo fmt` 的代码
**Then** CI 失败并明确报告差异

---

## Epic 2: Admin Authentication & First-Run Setup

让 admin 能完成首次访问的引导式设置（创建 admin 账号 + 配置 VPN 网段 + 创建首个用户）+ 后续登录、改密、注销操作。

### Story 2.1: 数据库 schema - users 表与 sessions 表

As a 项目开发者,
I want 用户与会话的持久化 schema,
So that 后续认证 Story 能直接读写数据.

**Acceptance Criteria:**

**Given** sqlx migrate 已就绪
**When** 创建 `20260511_120100_users.sql` 与 `20260511_120300_sessions.sql`
**Then** users 表含字段（id TEXT PRIMARY KEY、username TEXT UNIQUE、email TEXT UNIQUE、password_hash TEXT、role TEXT、status TEXT、must_change_password INTEGER、last_login_at INTEGER、created_at INTEGER、updated_at INTEGER）
**And** sessions 表含字段（id TEXT PRIMARY KEY、user_id TEXT FK、refresh_token_hash TEXT、ip_addr TEXT、user_agent TEXT、expires_at INTEGER、revoked_at INTEGER NULL、created_at INTEGER）
**And** 索引 `idx_users_email`、`idx_users_username`、`idx_sessions_user_id`、`idx_sessions_expires_at` 已创建
**And** `cargo sqlx prepare` 重新生成 `.sqlx/` 并提交

### Story 2.2: 实现 PasswordHasher (argon2id) 与 TokenIssuer (JWT RS256)

As a 项目开发者,
I want 密码哈希与 JWT 签发的服务实现,
So that 后续登录 Story 能调用统一接口.

**Acceptance Criteria:**

**Given** vpn-core 定义了 PasswordHasher 与 TokenIssuer trait
**When** 在 vpn-server/services 实现 Argon2idHasher（参数 m=64MB, t=3, p=2）与 JwtTokenIssuer（RS256）
**Then** PasswordHasher 单测：hash 一个密码、verify 正确密码返回 true、verify 错误密码返回 false
**And** TokenIssuer 单测：签发 Access Token（15min）+ Refresh Token（30 天），验证签名与过期时间

**Given** 服务端首次启动
**When** 未找到 JWT 私钥文件
**Then** 自动生成 RSA 2048 密钥对并保存到 `/var/lib/vpn/jwt_private.pem`（权限 600）
**And** 后续启动从文件加载，不再生成

### Story 2.3: 实现 user_repo 与 session_repo (SQLite)

As a 项目开发者,
I want Repository trait 的 SQLite 实现,
So that 业务服务能持久化用户与会话.

**Acceptance Criteria:**

**Given** vpn-core 定义了 UserRepository 与 SessionRepository trait
**When** 在 vpn-server/repositories 实现 `SqliteUserRepository` 与 `SqliteSessionRepository`
**Then** 提供方法：find_by_id / find_by_username / find_by_email / insert / update / delete（user）；create / find_by_token_hash / revoke / cleanup_expired（session）
**And** 使用 `#[sqlx::test]` 提供单元测试覆盖每个方法（in-memory 数据库 + 自动 migration）
**And** 重复 username/email 插入返回明确的 `AppError::DuplicateResource`

### Story 2.4: 实现 first-time-setup API 端点

As a IT 管理员,
I want 首次访问服务端时创建初始 admin 账号,
So that 我能开始使用管理后台（PRD FR3）.

**Acceptance Criteria:**

**Given** 系统首次部署且 users 表无 admin 记录
**When** POST `/api/v1/auth/first-time-setup` 携带 `{username, email, password}`
**Then** 创建 admin 账号（role=admin, status=active, must_change_password=false）
**And** 返回 ApiResponse success + 自动签发 Token

**Given** 已存在 admin 账号
**When** 再次调用 `/api/v1/auth/first-time-setup`
**Then** 返回 ApiResponse 错误码 2001（Forbidden）拒绝执行

### Story 2.5: 实现登录 API 与限速

As a IT 管理员或员工,
I want 用账号密码登录系统,
So that 我能获得操作所需的 Token（PRD FR6, FR10, FR11, FR56, FR57, FR58）.

**Acceptance Criteria:**

**Given** 用户已存在且状态为 active
**When** POST `/api/v1/auth/login` 携带 `{username, password}`
**Then** 验证密码正确后签发 Access Token（15min）+ Refresh Token（30 天）
**And** 更新 user.last_login_at
**And** 返回 ApiResponse success 含两个 Token

**Given** 同一 IP 在 1 分钟内调用登录 ≥ 5 次
**When** 第 6 次调用
**Then** tower-governor 中间件拦截，返回 HTTP 429 + 错误码 4001

**Given** 同一用户连续 5 次登录失败
**When** 第 6 次尝试（即使密码正确）
**Then** 返回错误码 1003（账号锁定），下次解锁时间 15 分钟后

**Given** 用户名不存在或密码错误
**When** 登录失败
**Then** 一律返回错误码 1001 "用户名或密码错误"（不区分两种情况）

### Story 2.6: 实现 Refresh / Logout API

As a 已登录用户,
I want 通过 Refresh Token 换取新的 Access Token，以及主动注销当前会话,
So that 长期使用无需重新登录 + 离开时安全注销（PRD FR9）.

**Acceptance Criteria:**

**Given** 持有有效 Refresh Token
**When** POST `/api/v1/auth/refresh` 携带 Refresh Token
**Then** 验证未撤销且未过期后签发新的 Access Token（不轮换 Refresh Token，简化 MVP）

**Given** 已登录用户
**When** POST `/api/v1/auth/logout`
**Then** 在 sessions 表标记当前 Refresh Token 为 revoked（设 revoked_at）
**And** 该 Refresh Token 后续无法换取 Access Token

### Story 2.7: 实现 AuthLayer + RequireAdmin extractor

As a 项目开发者,
I want 统一的 JWT 解析中间件与 admin 角色判定,
So that 后续 handler 只需声明 extractor 即获得认证.

**Acceptance Criteria:**

**Given** AuthLayer 已注册到 axum Router
**When** 请求携带有效 `Authorization: Bearer <token>`
**Then** 中间件解析 JWT 并将 CurrentUser 注入 Request extension
**And** handler 可通过 `CurrentUser` extractor 获取当前用户 ID 与角色

**Given** 请求缺失或携带过期/无效 Token
**When** 访问需要认证的端点
**Then** 中间件返回错误码 1001（缺失）或 1002（过期/无效）

**Given** 端点声明了 `RequireAdmin` extractor
**When** 非 admin 角色用户访问
**Then** 返回错误码 2001（需要 admin 角色）

### Story 2.8: 实现修改密码 API + 首次改密强制

As a 用户,
I want 修改自己的密码（且首次登录时被强制改密）,
So that 安全合规（PRD FR7, FR8, FR61）.

**Acceptance Criteria:**

**Given** 已登录用户
**When** POST `/api/v1/auth/change-password` 携带 `{old_password, new_password}`
**Then** 验证旧密码正确 + 校验新密码强度（≥ 8 位 + 字母 + 数字）
**And** 更新 password_hash + 设 must_change_password=false
**And** 撤销该用户所有 Refresh Token（强制重新登录）

**Given** 用户 must_change_password=true
**When** 访问 `/api/v1/auth/change-password` 以外的任何认证 API
**Then** 返回错误码 1006（必须先修改密码），前端可据此引导到改密页

**Given** 新密码不满足强度
**When** 修改密码
**Then** 返回错误码 1005 + 具体说明（如"密码至少 8 位且含字母与数字"）

### Story 2.9: 实现 GET /admin/system/info 端点

As a IT 管理员,
I want 在后台看到服务端的关键配置信息,
So that 我能确认部署正确并复制 VPN 网段等参数（PRD FR4）.

**Acceptance Criteria:**

**Given** admin 已登录
**When** GET `/api/v1/admin/system/info`
**Then** 返回 ApiResponse 含：`{version, vpn_subnet, server_public_key, server_endpoint, listen_port, started_at}`

**Given** 非 admin 用户
**When** 调用同一端点
**Then** 返回错误码 2001

### Story 2.10: 实现登录页 + Login Form

As a IT 管理员或员工,
I want 一个清晰的中文登录页,
So that 我能输入账号密码登录系统.

**Acceptance Criteria:**

**Given** 用户访问根路径或未登录访问 admin 路由
**When** 浏览器导航到 `/login`
**Then** 显示居中卡片含 vpn LOGO + "欢迎回来"标题 + 用户名 + 密码（含显示密码切换） + 登录按钮 + 灰色提示"忘记密码？请联系管理员"

**Given** 用户输入账号密码并提交
**When** 登录成功
**Then** 保存 Token 到 axios header + Refresh Token 到 httpOnly cookie + 跳转到 `/dashboard`

**Given** 登录失败
**When** 显示错误
**Then** 表单下方显示 Alert "用户名或密码错误"（不区分），密码字段自动清空

### Story 2.11: 实现 SetupWizard 组件 + 首次部署引导（2 步版本）

As a IT 管理员,
I want 一个 2 步引导完成首次部署设置,
So that 我能在 5 分钟内完成"部署 → 添加 admin → 配置网段"，进入仪表盘后用常规 `/users` 流程创建首个员工（PRD FR3 + UX-DR6, DR11）.

> 注：原 UX 设计的 3 步引导 Step 3（添加首个用户）依赖 Epic 3 的用户创建 API；为保持 Epic 2 独立可完成，本 Story 仅实现 Step 1 + Step 2，首个用户的"庆祝卡片"体验由 Epic 3 完成后通过 `/users` 页面的常规 CreateUserModal + ShareLinkModal 实现等价价值。

**Acceptance Criteria:**

**Given** 系统未配置 admin
**When** 访问任何路径
**Then** 系统自动跳转到 `/setup`

**Given** 已存在 admin
**When** 访问 `/setup`
**Then** 跳转到 `/login`

**Given** Setup Wizard Step 1（进度 ● ○）
**When** 填写 admin 账号信息（用户名/邮箱/密码 + 实时密码强度提示）并点击"下一步"
**Then** 调用 first-time-setup API 创建 admin + 自动登录 + 进入 Step 2

**Given** Setup Wizard Step 2（进度 ● ●）
**When** 填写 VPN 网段（默认 10.8.0.0/24，常见冲突给警告）+ 域名（自动填充当前 host）+ WG 监听端口（默认 51820）+ 点击"完成设置"
**Then** 服务端存储 system_config + 生成 WireGuard 服务端密钥对 + 跳转 `/dashboard`
**And** 仪表盘顶部显示"🎉 设置完成！下一步：[+ 创建第一个员工账号]"引导卡片，点击跳转 `/users` 直接打开 CreateUserModal

### Story 2.12: 实现仪表盘首页骨架 + 系统信息卡片

As a IT 管理员,
I want 登录后立即看到当前系统状态的概览,
So that 我每天打开后台能 5 秒内确认"今天有没有事"（UX-DR13）.

**Acceptance Criteria:**

**Given** admin 已登录
**When** 访问 `/dashboard`
**Then** 显示 ProLayout 框架内的 4 个 KPI 卡片（用户总数 / 在线节点 / 离线节点 / 今日新增），数据通过 React Query 拉取
**And** 显示系统信息卡片（VPN 网段 / 服务端公钥 / 监听端点 / 版本号，含一键复制）
**And** 显示"最近接入的节点"列表占位（无数据时 EmptyStateWithAction "还没有节点接入，把接入链接发给同事吧"）

### Story 2.13: 实现账号设置页（修改密码）

As a 已登录用户,
I want 修改自己的密码,
So that 定期安全更新.

**Acceptance Criteria:**

**Given** 已登录用户
**When** 点击顶栏头像菜单中"修改密码"
**Then** 跳转 `/account` 显示表单（当前密码 + 新密码 + 确认新密码 + 实时强度提示）

**Given** 用户提交修改密码
**When** API 返回成功
**Then** 显示 message success "密码修改成功，请重新登录"
**And** 3 秒后自动跳转 `/login`

---

## Epic 3: User Account Management

让 admin 能完整管理用户账号（CRUD + 重置密码 + 禁用 + 删除 + 复制接入链接）。

### Story 3.1: 实现 user_service.create + admin/users POST API

As a IT 管理员,
I want 创建新用户账号,
So that 新员工能拿到登录凭据（PRD FR14）.

**Acceptance Criteria:**

**Given** admin 已登录
**When** POST `/api/v1/admin/users` 携带 `{username, email}`（密码可选，不传则自动生成）
**Then** 系统自动生成 12 位强密码（含字母+数字+特殊字符）+ argon2id 哈希存储 + must_change_password=true
**And** 返回 ApiResponse 含 `{user: {...}, initial_password: "..."}`（密码明文一次性返回）

**Given** username 或 email 已存在
**When** 创建用户
**Then** 返回错误码 3003 "用户名/邮箱已存在"

**Given** 非 admin 用户
**When** 调用此端点
**Then** 返回错误码 2001

### Story 3.2: 实现用户列表 API + 搜索筛选

As a IT 管理员,
I want 分页查看所有用户并按状态/关键字搜索,
So that 在 100+ 用户场景下快速定位（PRD FR15, FR16, NFR-P5）.

**Acceptance Criteria:**

**Given** 数据库中有多个用户
**When** GET `/api/v1/admin/users?page=1&page_size=20&search=lin&status=active&order_by=created_at`
**Then** 返回 ApiResponse 含 `{items: [...], total: N, page, page_size}`
**And** 50 用户量场景下 p95 响应时间 ≤ 300ms

**Given** search 参数非空
**When** 查询
**Then** 模糊匹配 username 与 email（LIKE %search%）

### Story 3.3: 实现用户禁用/启用 API

As a IT 管理员,
I want 临时禁用某员工账号（保留数据）,
So that 员工请假/离职过渡期能阻止登录而不删除（PRD FR18）.

**Acceptance Criteria:**

**Given** 用户存在且状态为 active
**When** PATCH `/api/v1/admin/users/:id` `{status: "disabled"}`
**Then** users.status 更新为 disabled + 撤销该用户所有 Refresh Token

**Given** 用户状态为 disabled
**When** 该用户尝试登录
**Then** 返回错误码 1004 "账号已禁用，请联系管理员"

### Story 3.4: 实现重置密码 API

As a IT 管理员,
I want 重置任意用户的密码,
So that 员工忘密码时能快速恢复（PRD FR17）.

**Acceptance Criteria:**

**Given** 用户存在
**When** POST `/api/v1/admin/users/:id/reset-password`
**Then** 系统生成新强密码 + 更新哈希 + 设 must_change_password=true + 撤销该用户所有 Refresh Token
**And** 返回 ApiResponse 含 `{new_password: "..."}`（一次性显示）

**Given** admin 重置自己的密码
**When** 调用此 API
**Then** 系统拒绝，提示"请使用'修改密码'功能修改自己的密码"

### Story 3.5: 实现删除用户 API（级联清理）

As a IT 管理员,
I want 删除离职员工的账号及所有关联数据,
So that 符合合规要求（PRD FR19, FR60）.

**Acceptance Criteria:**

**Given** 普通用户存在
**When** DELETE `/api/v1/admin/users/:id`
**Then** 在单个数据库事务内删除：user 记录 + 该用户所有 sessions + 该用户所有 peers（含 WireGuard runtime 清理）

**Given** admin 尝试删除自己
**When** 调用此 API
**Then** 返回错误码 2002 "无法删除自己的账号"

**Given** 删除过程中任一步骤失败
**When** 事务回滚
**Then** 数据库与 WireGuard 运行时状态保持一致（不出现孤儿数据）

### Story 3.6: 实现用户管理页（ProTable + 搜索筛选）

As a IT 管理员,
I want 一个可视化用户管理列表,
So that 我能完成所有日常用户操作而无需 SSH（UX-DR14）.

**Acceptance Criteria:**

**Given** admin 已登录
**When** 访问 `/users`
**Then** 显示 ProTable 含列：用户名 / 邮箱 / 状态（标签：正常绿/已禁用灰） / 最后登录（相对时间） / 操作
**And** 表格上方有搜索框（防抖 300ms） + 状态筛选下拉 + 右上角"+ 新建用户"按钮
**And** 操作列：[详情] + [···] 下拉（重置密码 / 禁用-启用 / 删除红色 Popconfirm）

**Given** 数据库无用户
**When** 加载列表
**Then** 显示 `EmptyStateWithAction variant="users-empty"`（含"+ 创建第一个用户"按钮）

### Story 3.7: 实现 CreateUserModal

As a IT 管理员,
I want 一个弹窗快速创建用户,
So that 创建流程不超过 3 步.

**Acceptance Criteria:**

**Given** 用户点击"+ 新建用户"
**When** 显示 Modal
**Then** 表单含：用户名 * + 邮箱 * + 初始密码 *（含"🎲 生成"按钮自动填充 12 位强密码） + 提示文字"用户首次登录时需修改密码"

**Given** 用户填写完成并点"创建用户"
**When** API 调用成功
**Then** 关闭创建 Modal + 立即弹出 `ShareLinkModal` 显示接入链接 + 密码 + 一键复制按钮
**And** 表格自动刷新显示新用户

**Given** 用户名已存在
**When** 提交
**Then** 用户名字段下方红色提示"用户名已存在"，密码字段保留

### Story 3.8: 实现 ShareLinkModal + CopyLink 组件

As a IT 管理员,
I want 创建用户后一键复制接入链接 + 密码发给员工,
So that 完成"添加新员工"旅程末端（UX-DR7, UX-DR14）.

**Acceptance Criteria:**

**Given** 用户已创建
**When** ShareLinkModal 显示
**Then** 显示两行：接入链接（`https://<domain>/setup`） + 初始密码（明文）
**And** 显示"[📄 复制链接和密码]"按钮 + "[完成]"按钮

**Given** 用户点击"复制"按钮
**When** 复制内容为多行文本（含 link + 密码 + 简单使用说明）
**Then** 按钮 100ms 内变为"[✓ 已复制]" + message.success 提示 + 2 秒后按钮恢复

**Given** 浏览器 clipboard API 不可用
**When** 点击复制
**Then** 提供 fallback 文本框 + "请手动复制"提示

### Story 3.9: 实现 ResetPasswordModal

As a IT 管理员,
I want 重置用户密码后一键复制新密码,
So that 我能快速发给该员工.

**Acceptance Criteria:**

**Given** admin 在用户列表点击"重置密码"并确认
**When** API 返回新密码
**Then** 弹出 Modal 显示新密码（明文，monospace 字体） + "[复制密码]"按钮 + 提示"该密码仅显示一次，请立即发给用户"

### Story 3.10: 实现 EmptyStateWithAction 组件

As a 前端开发者,
I want 一个可复用的引导式空状态组件,
So that 所有列表/搜索无结果场景都有清晰下一步行动（UX-DR9）.

**Acceptance Criteria:**

**Given** 组件已实现
**When** 渲染 `<EmptyStateWithAction variant="users-empty" />`
**Then** 显示居中图标 + 标题"还没有用户" + 描述"创建第一个用户开始使用" + 主行动按钮"[+ 创建第一个用户]"

**Given** variant 为 `search-empty`
**When** 渲染
**Then** 显示"没找到匹配的内容，试试其他关键词"+"[清除搜索]"按钮

**Given** 组件支持自定义 props
**When** 通过 `title` / `description` / `action` 覆盖默认
**Then** 显示自定义内容

---

## Epic 4: VPN Tunnel & Client Connectivity

实现 VPN 数据平面 + 跨平台客户端，让用户能从 macOS/Linux/Windows 通过账号密码登录 + 自动连接 VPN + 节点间互通。

### Story 4.1: 实现 vpn-wireguard 基础 - WgApi 封装与密钥管理

As a 项目开发者,
I want vpn-wireguard crate 封装 defguard_wireguard_rs 的 WgApi,
So that 业务层用统一 API 操作 WireGuard.

**Acceptance Criteria:**

**Given** vpn-wireguard crate 已初始化
**When** 实现 `WireGuardApi` struct 封装 defguard_wireguard_rs + `WgPeerConfig` 类型 + `generate_keypair()`（x25519-dalek + OsRng）
**Then** 提供方法：create_interface / configure_peer / remove_peer / list_peers / shutdown
**And** 单元测试覆盖密钥生成（生成 100 次都不重复）+ peer config 序列化

**Given** 服务端首次启动
**When** system_config 表无 server_private_key
**Then** 自动生成 WireGuard 服务端密钥对 + 写入 system_config 表（明文 base64）

### Story 4.2: 实现 IP 池管理（vpn-wireguard/ip_pool.rs）

As a 项目开发者,
I want 从 CIDR 池中静态分配虚拟 IP,
So that 每个 peer 获得稳定 IP，重连不变（PRD FR22, FR23）.

**Acceptance Criteria:**

**Given** VPN 网段为 10.8.0.0/24
**When** 调用 `ip_pool.allocate(user_id)`
**Then** 从池中分配下一个可用 IP（跳过 .0 网络 / .1 服务端 / .255 广播）
**And** 同一 user_id 重复 allocate 返回同一 IP（静态绑定）

**Given** 池已耗尽（254 个全部分配）
**When** 再次 allocate
**Then** 返回 `AppError::IpPoolExhausted`

**Given** peer 被删除
**When** 调用 `ip_pool.release(ip)`
**Then** IP 重新可用于下次分配

### Story 4.3: 实现服务端 WireGuard 接口创建与启动恢复

As a IT 管理员,
I want 服务端启动时自动创建 WireGuard 接口并恢复所有 peers,
So that 重启后无需手动介入即可恢复服务（PRD FR5, NFR-R5）.

**Acceptance Criteria:**

**Given** 创建 `20260511_120200_peers.sql` migration
**When** 执行
**Then** peers 表含字段（id、user_id FK、device_name、wg_public_key UNIQUE、vpn_ip UNIQUE、endpoint、last_seen_at、status、os_info、created_at）+ 必要索引

**Given** 服务端首次启动
**When** wireguard_runtime/runtime.rs 调用 `create_interface(wg0, server_private_key, vpn_subnet)`
**Then** 系统创建 wg0 接口 + 配置服务端公私钥 + 监听 UDP 端口（51820 默认）

**Given** 服务端重启
**When** 启动序列执行 `reload_all_peers()`
**Then** 从 peers 表读取所有 status=active 的 peer + 逐个调用 WgApi.configure_peer
**And** 30 秒内完成所有 peer 配置恢复（NFR-R5）

### Story 4.4: 实现 boringtun timer task（关键陷阱规避）

As a 项目开发者,
I want 一个独立 tokio task 每 100ms 调用 update_timers,
So that 避免 boringtun 静默断线陷阱（NFR-R7）.

**Acceptance Criteria:**

**Given** vpn-wireguard 创建 boringtun Tunn 实例
**When** 启动 timer task
**Then** task 持续运行：每 100ms 调用 `tunn.update_timers(&mut buf)` 并处理返回的 TunnResult（WriteToNetwork → UDP send）

**Given** timer task 异常 panic
**When** 主进程检测
**Then** 自动重启 task 并记录 ERROR 日志（确保 task 永远在运行）

**Given** 服务端配置 metric "vpn_wg_timer_lag_ms"
**When** 单次 update_timers 耗时 > 50ms
**Then** 记录 WARN 日志 + metric 上报（便于排查）

### Story 4.5: 实现 peer 注册 API + IP 分配

As a 客户端用户,
I want 用 WireGuard 公钥注册节点,
So that 服务端能配置隧道（PRD FR21, FR22, FR24）.

**Acceptance Criteria:**

**Given** 已认证用户
**When** POST `/api/v1/peers/register` `{wg_public_key, device_name, os_info}`
**Then** 系统在事务内：(1) 从 ip_pool 分配 VPN IP（用户首次注册）或复用现有 IP（同一 user 二次注册）(2) 写入 peers 表 (3) 调用 WgApi.configure_peer
**And** 返回 ApiResponse 含 `{vpn_ip, server_public_key, server_endpoint, vpn_subnet}`

**Given** 同一 user_id 已注册过设备
**When** 再次注册（如重装客户端、新生成密钥对）
**Then** 更新现有 peers 记录的 wg_public_key + device_name + os_info（保留 vpn_ip 不变）

### Story 4.6: 实现 peer 心跳 API + 离线检测

As a 客户端 daemon,
I want 定期上报心跳让服务端知道我在线,
So that admin 后台能看到准确状态（PRD FR25, FR29, NFR-P6）.

**Acceptance Criteria:**

**Given** 已注册 peer
**When** POST `/api/v1/peers/heartbeat`（client header 含 endpoint）每 30 秒
**Then** 更新 peers.last_seen_at + peers.endpoint + peers.status=online
**And** 接口 p95 ≤ 100ms

**Given** 后台任务每 30 秒扫描
**When** peer.last_seen_at < (now - 90s) 且 status=online
**Then** 标记 status=offline

**Given** 心跳频率超过限速（6/分钟/Token）
**When** 触发
**Then** 返回 4001（客户端应退避，但不报错给用户）

### Story 4.7: 实现 peer 注销 API + 配置下载 API

As a 客户端用户,
I want 主动注销节点 + 下载当前 WireGuard 配置,
So that 离开时清理 + 手动配置 wg 工具时可用（PRD FR24, FR26）.

**Acceptance Criteria:**

**Given** 已认证用户
**When** DELETE `/api/v1/peers/me`
**Then** 调用 WgApi.remove_peer 移除服务端配置 + 从 peers 表标记 status=deleted（保留 IP 占用 24h 后释放）

**Given** 已注册 peer
**When** GET `/api/v1/peers/me/config`（Accept: text/plain）
**Then** 返回标准 WireGuard `.conf` 文本内容（含客户端 PrivateKey 占位 + Address + DNS + Peer 公钥 + Endpoint + AllowedIPs + PersistentKeepalive=25）
**And** Content-Disposition: attachment; filename="vpn.conf"

### Story 4.8: 实现 vpn-platform TunDevice trait 与三平台实现

As a 项目开发者,
I want 跨平台 TUN 抽象层,
So that vpn-cli 业务代码不直接处理三平台差异（NFR-C5）.

**Acceptance Criteria:**

**Given** vpn-platform crate 已初始化
**When** 定义 `TunDevice` trait（async fn recv / send / configure_ip / close）
**Then** 提供 Linux 实现（基于 tun-rs 配置 /dev/net/tun）+ macOS 实现（utun）+ Windows 实现（WinTun.dll 嵌入）

**Given** 在 ubuntu 上以 CAP_NET_ADMIN 权限运行测试
**When** 创建 TunDevice + configure_ip("10.8.0.5/24")
**Then** `ip addr` 显示 vpn-cli0 接口已配置

**Given** CI 三平台矩阵
**When** 编译 vpn-platform
**Then** Linux/macOS/Windows 三平台均编译通过（无 #[cfg] 错误）

### Story 4.9: 实现 vpn-platform CredentialStore trait 与三平台实现

As a 客户端 daemon,
I want 跨平台安全凭证存储,
So that Refresh Token 不以明文落盘（PRD FR35）.

**Acceptance Criteria:**

**Given** vpn-platform/credential.rs
**When** 定义 `CredentialStore` trait（save / load / delete）
**Then** 提供 macOS 实现（security-framework Keychain）+ Linux 实现（keyring crate libsecret）+ Windows 实现（Credential Manager）

**Given** 各平台测试环境
**When** save("vpn-server-url", "token-string") → load → delete
**Then** 三平台均成功且 token 在 OS 凭据库中可见

**Given** Linux 无 libsecret 服务
**When** save 失败
**Then** fallback 到加密文件（XSalsa20Poly1305，密钥派生自 user 主目录路径）

### Story 4.10: 实现 vpn-platform DaemonRuntime trait（系统服务集成）

As a 客户端用户,
I want 客户端 daemon 能开机自启 + 后台运行,
So that 我装好客户端后再也不用手动启动（PRD FR37）.

**Acceptance Criteria:**

**Given** vpn-platform/daemon.rs
**When** 定义 `DaemonRuntime` trait（install / uninstall / start / stop / status）
**Then** 提供 Linux 实现（systemd user service）+ macOS 实现（launchd LaunchAgent）+ Windows 实现（Windows Service）

**Given** 各平台
**When** 执行 `vpn-cli daemon install`
**Then** 系统注册 daemon 服务 + 开机自启 + 当前会话即启动
**And** `systemctl --user status vpn-cli` 或 `launchctl list | grep vpn-cli` 或 `sc query vpn-cli` 显示运行中

### Story 4.11: 实现 IPC（CLI ↔ daemon）

As a CLI 命令,
I want 与后台 daemon 通过 IPC 通信,
So that CLI 是无状态的，所有连接状态由 daemon 维护（AR-I2）.

**Acceptance Criteria:**

**Given** vpn-platform/ipc.rs
**When** 实现 Linux/macOS Unix Domain Socket + Windows Named Pipe + JSON-RPC 2.0 over length-prefixed frame
**Then** 定义 method 清单：`status` / `start` / `stop` / `login` / `logout` / `config`
**And** Socket/Pipe 路径权限严格（仅当前用户可读写）

**Given** daemon 已运行
**When** CLI 调用 `ipc::client::call("status", {})`
**Then** 通过 IPC 取回 daemon 当前状态（在线/离线/连接中、VPN IP、流量、运行时长）

**Given** daemon 未运行
**When** CLI 调用 IPC
**Then** 返回明确错误 "daemon 未运行，请执行 `vpn-cli daemon install` 安装"

### Story 4.12: 实现 vpn-cli api_client（服务端 HTTP 客户端）

As a 项目开发者,
I want 一个 vpn-cli 专用的 reqwest 客户端,
So that 所有服务端调用走统一封装.

**Acceptance Criteria:**

**Given** vpn-cli/api_client/ 模块
**When** 实现 ApiClient 含 base_url + auth_token，方法：login / refresh / logout / register_peer / heartbeat / get_my_config / delete_my_peer
**Then** 所有方法返回 `Result<T, AppError>`（解析 ApiResponse 信封）
**And** 单元测试用 mockito 覆盖成功/失败路径

### Story 4.13: 实现 vpn-cli login 命令

As a 客户端用户,
I want `vpn-cli login` 命令完成登录与首次接入,
So that 我能开始使用 VPN（PRD FR32, FR36）.

**Acceptance Criteria:**

**Given** 用户首次运行 `vpn-cli login admin@vpn.acme.com`
**When** 提示输入密码（rpassword 不回显）
**Then** 调用服务端 login API + 保存 Refresh Token 到 CredentialStore

**Given** API 返回 `must_change_password=true`
**When** 客户端检测到
**Then** 终端提示"[首次登录] 请设置新密码：" + 引导改密 + 调用 change-password API

**Given** 登录成功后
**When** daemon 检测到新登录
**Then** 自动生成 WG 密钥对（x25519-dalek + OsRng）+ 调用 register_peer API + 启动隧道
**And** 终端输出绿色 ✓ "已连接，您的虚拟 IP: 10.8.0.5"

### Story 4.14: 实现 vpn-cli daemon 主循环 - 隧道建立与维护

As a 客户端 daemon,
I want 维护与服务端的 VPN 隧道,
So that 后台默默工作（PRD FR41, FR42）.

**Acceptance Criteria:**

**Given** daemon 已启动 + 有效 Token + 已注册 peer
**When** 启动隧道
**Then** 创建 boringtun Tunn 实例（Arc<Mutex<Tunn>> 串行访问） + 创建 TunDevice + 创建 UDP socket 连接服务端 endpoint
**And** 启动 timer task（每 100ms 调用 update_timers）
**And** 启动主循环：(a) TUN.recv → Tunn.encapsulate → UDP.send；(b) UDP.recv → Tunn.decapsulate → TUN.send

**Given** 隧道建立成功
**When** 客户端 ping 服务端虚拟 IP（如 10.8.0.1）
**Then** 收到回应 + RTT < 50ms（本地环境测试）

**Given** 启动失败（如 TUN 权限不足）
**When** daemon 报错
**Then** IPC 状态返回 `error` + 明确错误消息（"创建 TUN 失败：需要 CAP_NET_ADMIN 权限"）

### Story 4.15: 实现 vpn-cli start / stop / status / logout 命令

As a 客户端用户,
I want CLI 命令控制 daemon 行为,
So that 我能脚本化或手动控制（PRD FR33, FR34, FR38）.

**Acceptance Criteria:**

**Given** daemon 已运行
**When** `vpn-cli start`
**Then** 通过 IPC 通知 daemon 启动隧道（若已运行则提示"已连接"）

**Given** daemon 已建立隧道
**When** `vpn-cli stop`
**Then** 通过 IPC 通知 daemon 关闭隧道（保留 Token，下次 start 可直接连）

**Given** daemon 任意状态
**When** `vpn-cli status`
**Then** 人类可读输出含"✓ 已连接 / ✗ 已断开 / ⏳ 连接中"+ 服务端 + 虚拟 IP + 流量 + 运行时长

**Given** 任意命令加 `--json` 参数
**When** 执行
**Then** 改为输出 JSON 格式（不带颜色码，适合脚本解析）

**Given** 已登录
**When** `vpn-cli logout`
**Then** 调用 server logout API + 删除本地 Refresh Token + 停止 daemon 隧道

### Story 4.16: 实现客户端自动重连（指数退避）

As a 员工,
I want 客户端在网络/服务端波动时自动恢复,
So that 我感受不到断线（NFR-R3, UX-DR Journey 2.2）.

**Acceptance Criteria:**

**Given** daemon 隧道建立成功后服务端不可达（如服务端重启）
**When** 心跳/数据包持续失败 30 秒
**Then** daemon 进入重连模式：退避 1s, 2s, 4s, 8s, 16s, 32s, 最大 60s

**Given** 重连尝试中
**When** 服务端恢复
**Then** 隧道在 ≤ 3 秒内恢复 + 状态变为 online

**Given** 100 次连续重连测试（脚本模拟）
**When** 统计成功率
**Then** ≥ 99 次成功（NFR-R3）

### Story 4.17: 实现客户端网络变化检测

As a 员工,
I want 切换 Wi-Fi / 唤醒后自动重连,
So that 我移动办公时无感（PRD FR44）.

**Acceptance Criteria:**

**Given** daemon 运行中
**When** 系统网络接口变化（Linux: netlink RTMGRP_LINK；macOS: SCDynamicStore；Windows: NotifyAddrChange）
**Then** daemon 检测到事件 + 触发 reconnect 流程（关闭旧 UDP socket + 重新解析服务端域名 + 建立新隧道）

**Given** 切换 Wi-Fi 测试
**When** 客户端从 Wi-Fi A 切到 Wi-Fi B
**Then** ≤ 3 秒内隧道恢复 + 虚拟 IP 不变

### Story 4.18: 实现 ConnectionGuide 组件 + /setup 公开页

As a 员工,
I want 一个公开的接入引导页,
So that 我能自助下载客户端并知道如何连接（PRD FR30, UX-DR8, DR17）.

**Acceptance Criteria:**

**Given** 任何人（无需登录）
**When** 访问 `/setup`
**Then** 显示 "欢迎接入 vpn / 3 步完成接入"标题 + 3 个步骤卡片

**Given** Step 1
**When** useOsDetect 检测到当前 OS
**Then** 自动显示对应平台下载按钮（macOS .pkg / Windows .msi / Linux .deb），附文件大小
**And** 其他平台列在下方可点击切换

**Given** Step 3 显示 CLI 命令
**When** 用户点击 [📄]
**Then** 复制 `vpn-cli login <服务端域名>` 到剪贴板 + 反馈 ✓

**Given** 调用 `GET /api/v1/public/system/info`
**When** 服务端返回当前域名
**Then** 命令中的域名自动填充

---

## Epic 5: Peer Monitoring & Audit Visibility

让 admin 能在后台可视化监控所有节点状态 + 查询审计日志 + 独立完成故障排查（不需 SSH）。

### Story 5.1: 数据库 schema - audit_logs 表

As a 项目开发者,
I want audit_logs 表 schema,
So that 后续 audit 中间件能写入日志.

**Acceptance Criteria:**

**Given** sqlx migrate 已就绪
**When** 创建 `20260511_120400_audit_logs.sql`
**Then** audit_logs 表含字段（id TEXT、user_id TEXT NULL、username TEXT、action TEXT、resource TEXT、ip_addr TEXT、user_agent TEXT、metadata TEXT JSON、created_at INTEGER）
**And** 索引 `idx_audit_logs_created_at`、`idx_audit_logs_user_id`、`idx_audit_logs_action`

### Story 5.2: 实现 audit_service + AuditLogLayer 中间件

As a IT 管理员,
I want 系统自动记录所有写操作的审计日志,
So that 故障排查与合规追溯有数据（PRD FR48, FR49, FR50）.

**Acceptance Criteria:**

**Given** AuditLogLayer 已注册
**When** 任何 POST/PATCH/DELETE 请求完成
**Then** 异步写入 audit_logs（含 user_id from JWT、username、action 推断、resource path、ip_addr、user_agent、status_code）
**And** 写入失败不阻塞响应，降级输出 stderr WARN 日志

**Given** 登录失败事件
**When** auth_service 显式调用 `audit_service.log_login_attempt(username, success, reason, ip)`
**Then** 写入 audit_logs 含 action="login_failed" + metadata 含失败原因

### Story 5.3: 实现审计日志清理（6 月保留）

As a 项目维护者,
I want 自动清理超过 6 月的审计日志,
So that 数据库不会无限增长（PRD FR52, NFR-S10）.

**Acceptance Criteria:**

**Given** 后台任务每天 03:00 执行
**When** 扫描 audit_logs.created_at < (now - 180 天)
**Then** 删除这些记录 + 记录 INFO 日志含清理条数

**Given** 配置 `VPN_AUDIT_RETENTION_DAYS=365`（环境变量）
**When** 任务执行
**Then** 改为按 365 天保留

### Story 5.4: 实现审计日志查询 API

As a IT 管理员,
I want 多维度筛选审计日志,
So that 快速定位特定事件（PRD FR51）.

**Acceptance Criteria:**

**Given** admin 已登录
**When** GET `/api/v1/admin/audit-logs?from=<ts>&to=<ts>&user_id=<id>&action=<type>&page=1&page_size=20`
**Then** 返回分页结果（按 created_at desc）+ 总数
**And** 50 万条日志场景下查询 p95 ≤ 500ms（索引保证）

**Given** 不传任何筛选参数
**When** 调用
**Then** 默认返回最近 7 天数据（避免全表扫描）

### Story 5.5: 实现 admin peer 列表 API + 强制下线 API

As a IT 管理员,
I want 查看所有节点状态并能强制下线,
So that 处理异常节点（PRD FR27, FR28）.

**Acceptance Criteria:**

**Given** admin 已登录
**When** GET `/api/v1/admin/peers?page=1&page_size=20&search=&status=`
**Then** 返回所有 peers 含 status / vpn_ip / last_seen_at / endpoint / os_info / device_name / user(username, email)
**And** 支持按 username / device_name 搜索 + 按 status 筛选

**Given** admin 选择强制下线
**When** DELETE `/api/v1/admin/peers/:id`
**Then** 调用 WgApi.remove_peer + 更新 peers.status=force_removed
**And** 该节点客户端下次心跳收到 401 + 提示重新登录

### Story 5.6: 实现 NodeStatusDot 组件

As a 前端开发者,
I want 一个统一的节点状态色点组件,
So that 所有节点列表显示一致（UX-DR5）.

**Acceptance Criteria:**

**Given** 组件 `<NodeStatusDot status="online" />`
**When** 渲染
**Then** 显示 8px 绿色圆点（#52C41A） + 14px 文字"在线"
**And** 包含 aria-label="节点状态：在线"

**Given** status 取值 connecting / offline / error
**When** 渲染
**Then** 分别显示黄/灰/红色点 + 对应文字（"连接中" / "离线" / "异常"）

**Given** 传入 lastSeen prop
**When** hover
**Then** 显示 tooltip "最后心跳：X 分钟前"

**Given** size prop 为 sm / md / lg
**When** 渲染
**Then** 色点 6/8/12px、文字 12/14/16px

### Story 5.7: 实现节点管理页（ProTable + 静态轮询）

As a IT 管理员,
I want 可视化节点列表 + 强制下线操作,
So that 完成故障排查旅程（UX-DR15）.

**Acceptance Criteria:**

**Given** admin 访问 `/peers`
**When** 加载页面
**Then** 显示 ProTable 含列：状态（NodeStatusDot） / 设备名 / 所属用户 / 虚拟 IP / 最后心跳（相对时间） / 公网 endpoint / 操作
**And** 表格上方有搜索框 + 状态筛选下拉
**And** 操作列：[详情] + [强制下线] (红色 + Popconfirm)
**And** 通过 React Query 每 10 秒静态轮询（refetchInterval: 10000）

**Given** 无节点
**When** 加载
**Then** 显示 EmptyStateWithAction variant="peers-empty"

### Story 5.8: 实现审计日志页（ProTable）

As a IT 管理员,
I want 审计日志查询页,
So that 快速定位故障原因（UX-DR16）.

**Acceptance Criteria:**

**Given** admin 访问 `/audit-logs`
**When** 加载页面
**Then** 显示 ProTable 含列：时间（绝对+相对悬停） / 用户 / 类型（Tag 标签）/ 详情（展开行显示完整 metadata）
**And** 表格上方有 [时间范围 ▾] + [类型 ▾] + [用户名搜索] 筛选
**And** 默认显示最近 7 天

**Given** 筛选条件变化
**When** 更新筛选
**Then** URL 同步参数（便于分享链接）

### Story 5.9: 完善仪表盘 - KPI 卡片与待办告警

As a IT 管理员,
I want 仪表盘显示当前关键指标与待办告警,
So that 我每天 5 秒确认"今天有没有事"（UX-DR13）.

**Acceptance Criteria:**

**Given** admin 访问 `/dashboard`
**When** 加载
**Then** 4 KPI 卡片显示实时数字：用户总数 / 在线节点 / 离线节点 / 今日新增（最近 24 小时新加入的 peer）

**Given** 存在离线超 10 分钟的节点
**When** 加载仪表盘
**Then** 显示 Alert 警告"⚠ 节点 X 已离线 12 分钟"+ "[处理]"按钮 → 跳转节点详情

**Given** 最近接入的 peer 列表
**When** 加载
**Then** 显示最近心跳的 10 个 peer（NodeStatusDot + 设备名 + 虚拟 IP + 时间）+ "查看全部 →" 链接到 `/peers`

---

## Epic 6: Production Release

让项目能正式发布到 GitHub Releases + Docker Hub，含完整文档、E2E 测试、客户端安装包。

### Story 6.1: 编写完整 README（中文 + 截图）

As a 潜在用户,
I want 一个清晰的中文 README,
So that 我能在 5 分钟内决定是否使用并完成部署（NFR-O7）.

**Acceptance Criteria:**

**Given** 项目根目录
**When** 编写 README.md
**Then** 包含：项目简介 1 段 + 关键截图（管理后台 + 客户端 status）+ 5 分钟 quickstart（docker run 命令）+ 系统要求 + 客户端下载链接 + 完整文档链接 + License + 截图均为实际功能截图

**Given** 5 位无 VPN 经验的开发者（友人/同事）
**When** 按 README quickstart 部署
**Then** ≥ 4 位（80%）能在 30 分钟内完成部署 + 首次客户端连接（PRD §3 验收）

### Story 6.2: 编写 docs/（部署、API、CLI、故障排查）

As a 用户,
I want 详细使用文档,
So that 我能查到所有功能与故障排查（NFR-M5, NFR-O7）.

**Acceptance Criteria:**

**Given** docs/ 目录
**When** 编写以下文件
**Then**：
  - `docs/deployment.md`：Docker / systemd 两种部署方式 + 配置参数说明 + 域名/防火墙要求
  - `docs/api.md`：完整 REST API 参考（每个端点：路径/方法/请求/响应/错误码/示例）
  - `docs/client-cli.md`：CLI 所有命令 + 选项 + 退出码 + 平台差异
  - `docs/troubleshooting.md`：10 个常见问题排查（HTTPS 申请失败 / 客户端连不上 / 速度慢 / ...）
**And** 所有文档含相关截图

### Story 6.3: 实现客户端 .deb / .rpm 构建脚本

As a Linux 用户,
I want 标准 Linux 安装包,
So that 一行命令完成客户端安装（PRD FR31, AR-D3）.

**Acceptance Criteria:**

**Given** vpn-cli 二进制已构建
**When** 执行 `installers/linux/build-deb.sh`
**Then** 生成 `vpn-cli_<version>_amd64.deb`（含 vpn-cli 可执行文件 + systemd user service unit + man page 占位 + post-install 脚本自动 enable daemon）

**Given** 已生成的 .deb
**When** 在 Ubuntu 22.04 执行 `sudo dpkg -i vpn-cli_*.deb`
**Then** `vpn-cli --version` 可用 + systemd user service `vpn-cli.service` 已注册

**Given** `installers/linux/build-rpm.sh`
**When** 在 Fedora/CentOS 上构建并安装
**Then** 类似 .deb 流程通过

### Story 6.4: 实现客户端 macOS .pkg 构建脚本

As a macOS 用户,
I want 标准 .pkg 安装包,
So that 双击安装（PRD FR31）.

**Acceptance Criteria:**

**Given** vpn-cli 二进制已构建（arm64 + x86_64 universal）
**When** 执行 `installers/macos/build-pkg.sh`
**Then** 生成 `vpn-cli-<version>.pkg`（含可执行文件 → /usr/local/bin + launchd LaunchAgent plist → ~/Library/LaunchAgents）

**Given** 在 macOS 13+ 双击 .pkg
**When** 按提示输入管理员密码完成安装
**Then** `vpn-cli --version` 在终端可用 + LaunchAgent 已注册

### Story 6.5: 实现客户端 Windows .msi 构建脚本

As a Windows 用户,
I want 标准 .msi 安装包,
So that 双击安装（PRD FR31）.

**Acceptance Criteria:**

**Given** vpn-cli.exe 已构建
**When** 执行 WiX `candle` + `light` 生成 `vpn-cli-<version>.msi`
**Then** MSI 包含 vpn-cli.exe + WinTun.dll + Windows Service 注册 + 添加到 PATH

**Given** 在 Windows 10 1809+ 以管理员权限双击 .msi
**When** 完成安装
**Then** PowerShell `vpn-cli --version` 可用 + `sc query vpn-cli` 显示服务存在

### Story 6.6: 实现 3 个 E2E 集成测试

As a 项目维护者,
I want 完整 E2E 测试覆盖核心旅程,
So that release 前自动验证基础功能（NFR-M2）.

**Acceptance Criteria:**

**Given** `docker/docker-compose.test.yml`
**When** 启动 server + client-a + client-b + test-runner 容器
**Then** 容器互相可见，client 通过 server 镜像内嵌的 vpn-cli 连接

**Given** E2E 1: 部署 + Setup Wizard
**When** test-runner 调用 first-time-setup API + verify 配置
**Then** admin 创建成功 + system_config 完整

**Given** E2E 2: 添加用户 + 客户端连接
**When** test-runner 创建用户 + client-a 用该账号 login + ping client-a 虚拟 IP（10.8.0.x）
**Then** ping 成功 + 后台 peer 列表显示 client-a online

**Given** E2E 3: 服务端重启自动重连
**When** 用例完成 E2E 2 后重启 server 容器，等待 30s
**Then** client-a 自动重连成功 + ping 仍通

**Given** `just test-e2e` 或 CI 调用
**When** 三个 E2E 全部通过
**Then** 输出绿色总结，否则 CI 红灯

### Story 6.7: 配置 GitHub Actions Release Pipeline

As a 项目维护者,
I want 打 tag 自动构建并发布 release,
So that 减少手动出错（AR-D5）.

**Acceptance Criteria:**

**Given** `.github/workflows/release.yml`
**When** 推送 tag 形如 `v0.1.0`
**Then** CI 触发流程：
  - 三平台构建 vpn-cli 二进制 + 调用 installers 脚本生成 .deb/.rpm/.pkg/.msi
  - 构建 Docker 镜像 `ghcr.io/<owner>/vpn:0.1.0` 并 push
  - 创建 GitHub Release 含 CHANGELOG + 所有安装包附件
  - 自动更新 README 中的"最新版本"徽章

### Story 6.8: 单元测试覆盖率达标（≥ 70%）

As a 项目维护者,
I want 核心模块测试覆盖率 ≥ 70%,
So that 后续重构不破坏功能（NFR-M1）.

**Acceptance Criteria:**

**Given** 已实现 Epic 1-5 所有代码
**When** 运行 `cargo llvm-cov --workspace --all-features --summary-only`
**Then** 核心模块（vpn-core / vpn-wireguard / vpn-server services + repositories）行覆盖率 ≥ 70%

**Given** CI 已配置 llvm-cov 步骤
**When** PR 触发
**Then** 报告显示覆盖率 + 若下降超过 2% 警告（不阻塞合并）

### Story 6.9: 安全审计 + 5 位真实用户部署测试

As a 项目维护者,
I want 发布前完成安全审计 + 真实用户测试,
So that v0.1.0 质量可控（PRD §3 业务成功标准）.

**Acceptance Criteria:**

**Given** 项目代码已就绪
**When** 执行安全审计清单：
  - `cargo audit` 无 CVE 高危
  - `npm audit --omit=dev` 无 high+
  - 检查暴露端口（仅 80, 443, 51820）
  - grep 日志中无 password / refresh_token 明文
  - Dockerfile USER 非 root + AmbientCapabilities 最小化
**Then** 全部通过

**Given** 5 位真实测试者（IT 友人）按 README 部署
**When** 30 分钟内
**Then** ≥ 4 位（80%）完成部署 + 首次客户端连接成功 + 提交反馈表
**And** 收集到的 P0 问题在 v0.1.0 release 前修复
