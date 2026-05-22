---
stepsCompleted: [step-01-init, step-02-discovery, step-02b-vision, step-02c-executive-summary, step-03-success, step-04-journeys, step-05-domain, step-06-innovation, step-07-project-type, step-08-scoping, step-09-functional, step-10-nonfunctional, step-11-polish, step-12-complete]
releaseMode: phased
classification:
  projectType: api_backend + cli_tool
  domain: network_security_infrastructure
  complexity: high
  projectContext: greenfield
inputDocuments:
  - research/technical-rust-vpn-openvpn-research-2026-05-11.md
  - research/technical-vpn-protocol-architecture-research-2026-05-11.md
  - research/technical-rust-vpn-security-integration-research-2026-05-11.md
  - research/technical-rust-vpn-architecture-patterns-research-2026-05-11.md
  - research/technical-rust-vpn-implementation-strategy-research-2026-05-11.md
  - research/technical-rust-vpn-risk-assessment-research-2026-05-11.md
researchCount: 6
briefCount: 0
brainstormingCount: 0
projectDocsCount: 0
workflowType: 'prd'
---

# Product Requirements Document - vpn

**Author:** Shangguanjunjie
**Date:** 2026-05-11
**Status:** MVP 规划完成（Phase 1：3–5 周）
**Release Mode:** 分阶段交付（MVP / Growth / Vision）

---

## Table of Contents

1. [Executive Summary](#executive-summary)
2. [Project Classification](#project-classification)
3. [Success Criteria](#success-criteria) — 用户/业务/技术成功标准与可测量指标
4. [Product Scope](#product-scope) — MVP / Growth / Vision 功能清单
5. [User Journeys](#user-journeys) — IT 管理员（老王）与员工（小李）的核心旅程
6. [Domain-Specific Requirements](#domain-specific-requirements) — 合规、技术约束、风险缓解
7. [API Backend & CLI Tool Specific Requirements](#api-backend--cli-tool-specific-requirements) — API 端点、CLI 命令、数据契约
8. [Project Scoping & Phased Development](#project-scoping--phased-development) — MVP 策略与风险缓解
9. [Functional Requirements](#functional-requirements) — 63 项功能能力契约（MVP 49 / G 8 / V 6）
10. [Non-Functional Requirements](#non-functional-requirements) — 43 项质量约束（MVP 38 / G 5）

> **使用指南：**
> - **下游 UX 设计**：聚焦 §5（用户旅程） + §9（功能需求）
> - **下游架构设计**：聚焦 §6（领域约束） + §7（API/CLI 规范） + §10（非功能需求）
> - **下游 Epic 拆分**：聚焦 §9（功能需求 + MVP/G/V 标签）

---

## Executive Summary

本项目构建一款**面向中小型企业（20–200 人）的轻量级自托管异地组网 VPN 系统**，使分布在不同地理位置的办公室、远程员工设备能够组成统一的虚拟局域网，安全访问公司内部资源。系统基于 Rust 实现，参考 OpenVPN 的双信道架构思路，数据平面采用 WireGuard 协议（Cloudflare boringtun），控制平面提供账号密码认证的 REST API 和中文管理后台 UI。目标用户为企业 IT 管理员（部署运维）和普通员工（终端使用）；解决的核心问题是：现有方案要么过于极客（innernet 命令行邀请码）、要么部署沉重（NetBird 5 组件）、要么数据出境且付费（Tailscale），中小企业缺乏一款"轻量易用 + 自托管 + 中文优先 + 稳定可靠"的方案。

### What Makes This Special

**核心差异化：** 5 分钟即可完成服务端搭建（单容器一条命令），员工客户端账号密码登录后自动连接，无需 IT 介入下发配置。

**核心洞察：** 中小企业需要的不是更强的协议或更花哨的功能，而是**稳定运行不出问题 + 中文友好的运维体验**。技术栈选择 WireGuard + Rust + Axum 不是为追新，而是这套组合在同等稳定性下需要的运维介入最少（boringtun 已在 Cloudflare/Mullvad 数百万设备验证；Rust 内存安全消除一类运行时崩溃；Axum 单二进制无依赖）。

**用户选择理由（vs 现有方案）：**

- **vs innernet**：图形化管理后台 + 账号密码登录（替代命令行邀请码）
- **vs NetBird**：单进程部署（替代 5 组件控制平面 + Signal + Relay）
- **vs Tailscale**：完全自托管 + 数据不出境 + 永久免费
- **vs 商业 SD-WAN（华为等）**：成本降低 95%+（$10/月 vs 数万元）

## Project Classification

- **项目类型：** API 后端（Axum REST + WebSocket）+ CLI 客户端工具（跨平台 VPN 守护进程）+ Web 管理后台
- **领域：** 网络安全基础设施 / 企业组网
- **复杂度：** 高（涉及加密协议、TUN 虚拟网卡、跨平台网络编程、路由转发）
- **项目背景：** 绿地项目（无既有系统约束）

## Success Criteria

### User Success

**IT 管理员（部署/运维角色）：**

- **零文档部署**：依照 README 一条命令完成部署，30 分钟内打通首个客户端连接
- **可视化日常运维**：通过后台 UI 完成 100% 日常操作（添加/删除账号、查看节点状态、踢下线），无需 SSH 进服务器
- **故障可自诊断**：客户端连接失败时，后台 UI 显示明确错误原因（错误密码 / 网络不通 / 服务端拒绝），管理员可独立排障
- **零运维介入新员工接入**：员工自助安装客户端 + 输入账号密码即可连接，IT 无需下发配置文件

**普通员工（终端用户）：**

- **首次连接 ≤ 3 分钟**：从下载客户端到成功连接 VPN 不超过 3 分钟
- **断线无感**：网络切换/服务端重启后客户端自动重连，重连成功率 ≥ 99%
- **像在公司一样**：连接成功后可直接访问公司内网资源（域名/IP），无需额外配置

### Business Success

**3 个月内（MVP 完成）：**

- 第一家试点企业（5–10 人）稳定使用 ≥ 30 天
- GitHub Star ≥ 50（验证开源吸引力）

**12 个月内：**

- 累计部署企业数 ≥ 20 家（自建/试点）
- 单实例稳定运行节点数 ≥ 50 个
- 文档 + Issue 响应使新用户首次部署成功率 ≥ 80%

### Technical Success

- **稳定性**：客户端断线自动重连成功率 ≥ 99%；服务端连续运行 ≥ 30 天不重启
- **可用性**：月度可用性 ≥ 99.5%（每月不可用 ≤ 3.6 小时）
- **性能**：单连接吞吐 ≥ 100 Mbps；隧道建立时间 ≤ 10 秒
- **资源占用**：50 节点场景下服务端 CPU < 30%，内存 < 1 GB
- **跨平台**：Linux/macOS/Windows 三平台客户端均可用，功能一致

### Measurable Outcomes

| 指标 | MVP 目标 | 验证方式 |
|------|---------|---------|
| 首次部署到首个客户端连接时间 | ≤ 30 分钟 | 5 位无 VPN 经验者实测平均值 |
| 客户端断线重连成功率 | ≥ 99% | 服务端重启/网络切换场景下连续 100 次测试 |
| 单实例并发节点数 | ≥ 50 | 压测脚本验证 |
| 服务端 7×24 连续运行 | ≥ 30 天 | 试点企业实际运行日志 |
| 后台 UI 功能完整度 | 涵盖所有 MVP 日常操作 | 不需 SSH 即完成 100% 管理任务 |

## Product Scope

### MVP - Minimum Viable Product（3–5 周交付）

**核心隧道（Hub-and-Spoke 模式）：**

- 服务端基于 boringtun + tun-rs 提供 WireGuard 隧道
- 客户端连接服务器后获得虚拟 IP，节点之间通过服务端转发互通

**账号密码认证：**

- argon2id 密码哈希存储
- JWT Access Token（15min）+ Refresh Token（30 天）登录
- 防爆破限速（5 次/分钟）

**后台管理 UI（中文）：**

- 登录页 + 仪表盘
- 用户管理（CRUD：增删改查、重置密码、禁用账号）
- 节点列表（在线状态、虚拟 IP、最后心跳时间、强制下线）
- 服务端配置查看（VPN 网段、服务端公钥、端点）

**跨平台客户端 CLI：**

- Linux / macOS / Windows
- 命令：`vpn-cli login` / `start` / `stop` / `status`
- 账号密码登录后自动获取 WireGuard 配置并启动隧道
- 断线自动重连

**部署：**

- Docker 单容器部署（Axum + SQLite，零外部依赖）
- 自带 ACME 自动 HTTPS（rustls-acme）
- 一份 README，30 分钟内可走完全流程

### Growth Features (Post-MVP, 4–6 周)

- 流量统计（按用户/按节点）
- 节点状态实时推送（WebSocket，免轮询）
- 角色权限（Admin / Operator / User）
- 审计日志（登录、配置变更）
- 监控集成（Prometheus metrics + Grafana 看板）
- 客户端 GUI（Tauri，替代 CLI）
- PostgreSQL 支持（替代 SQLite 用于大规模部署）

### Vision (Future)

- **Mesh 直连模式**：节点间 P2P 直连（NAT 穿透），不再经服务器转发
- **多服务端集群**：HA 高可用，主备切换
- **路由策略**：按用户/部门配置可访问的子网范围
- **SSO 集成**：LDAP / OAuth2 / SAML 接入企业 IDP
- **移动端**：iOS / Android 客户端

## User Journeys

### Persona 1：老王（IT 管理员，30 人广告公司技术主管）

**背景：** 老王在一家 30 人的广告公司担任 IT 主管，公司去年开了上海分公司（5 人），最近又有 3 个设计师转远程办公。原本的"VPN"是老王每周帮员工手动改 hosts、远程桌面进公司电脑，疲于奔命。看到老板要求"远程也能像在公司一样"，他急需一套靠谱的异地组网方案。

---

#### Journey 1.1：首次部署（Happy Path）

**Opening Scene：** 周五下午，老王打开 Linux VPS 控制台，准备试一下 GitHub 上找到的这个 Rust VPN 项目。

**Rising Action：**

1. 复制 README 里的 `docker run` 命令，5 分钟后服务跑起来了
2. 浏览器打开 `https://vpn.acme.com:8443`，自动 HTTPS 证书已申请好
3. 第一次访问引导他设置 admin 账号密码，2 分钟搞定
4. 后台 UI 显示"等待客户端连接"，他下载 macOS 客户端到自己电脑
5. 运行 `vpn-cli login admin@vpn.acme.com`，输入密码
6. 客户端提示"已连接，虚拟 IP: 10.8.0.2"

**Climax：** 老王尝试 `ping 10.8.0.1`（服务端虚拟 IP），通了！再尝试 `ping 192.168.1.100`（公司内网打印机），也通了！

**Resolution：** 老王看了眼时间，从开始部署到验证完毕共 22 分钟。他在便签上写下"周一给全员发部署指南"。

**揭示的需求：**

- 一条 docker run 命令完成部署
- 首次访问引导式 admin 初始化
- 自动 ACME HTTPS（无需手动配证书）
- 客户端 CLI 简单命令（login + 自动连接）
- 部署后立即可验证连通性

---

#### Journey 1.2：日常运维 - 添加新员工（Happy Path）

**Opening Scene：** 周一早上，HR 在群里发"今天入职 2 位新同事，请 IT 协助开通账号"。

**Rising Action：**

1. 老王打开 VPN 后台 UI
2. 点击"用户管理 → 添加用户"
3. 填写：用户名、邮箱、初始密码（系统自动生成 12 位随机串）
4. 点"创建"，复制系统生成的"员工接入指南"链接
5. 把链接和密码发给新员工

**Climax：** 老王回到工位继续工作。10 分钟后看到后台"在线节点"列表里出现了新员工的虚拟 IP。

**Resolution：** 整个过程不超过 3 分钟，老王再也不用手动改 WireGuard 配置文件了。

**揭示的需求：**

- 用户管理 CRUD（增删改查）
- 自动生成符合强度要求的密码
- 系统生成的"员工接入指南"链接（含下载/配置说明）
- 后台可视化在线节点状态

---

#### Journey 1.3：故障排查（Edge Case）

**Opening Scene：** 下午 3 点，远程设计师小李在群里发"突然连不上 VPN 了，急用！"

**Rising Action：**

1. 老王打开 VPN 后台 UI
2. 查"用户管理"列表，看到小李账号状态：正常
3. 切到"节点列表"，看到小李的节点：最后心跳 12 分钟前，状态"离线"
4. 切到"登录日志"，看到小李刚才尝试登录 5 次，全部失败，原因"密码错误"
5. 老王在群里告诉小李"密码错了"

**Climax：** 小李说"哦我刚换密码忘了"，老王在后台"重置密码"，生成新密码发给小李。

**Resolution：** 2 分钟后小李回复"连上了"。老王全程没碰 SSH。

**揭示的需求：**

- 后台显示登录失败日志（含失败原因）
- 节点状态实时更新（心跳超时检测）
- 一键重置用户密码功能

---

### Persona 2：小李（远程设计师，普通员工）

**背景：** 小李是公司的视觉设计师，去年因为孩子上学搬到了苏州，每天远程办公。她的痛点：访问公司设计资源库（10GB 的素材包）必须先用老式 VPN 连上跳板机再 SFTP 下载，慢且经常断。

---

#### Journey 2.1：首次连接（Happy Path）

**Opening Scene：** 老王在工作群发了"新 VPN 部署指南：vpn.acme.com/setup"，让大家自己装。小李有点紧张，她不太懂技术。

**Rising Action：**

1. 小李打开链接，看到中文指南，第一步"下载客户端"，自动识别她是 macOS，直接给了 .pkg 下载链接
2. 双击安装，提示"需要管理员权限"（一次），点同意
3. 安装完打开终端（按指南截图操作），输入 `vpn-cli login lin@acme.com`
4. 提示输入密码，输入老王发给她的初始密码
5. 终端显示"首次登录请修改密码"，她改了一个好记的密码
6. 终端显示"已连接，你的虚拟 IP: 10.8.0.5，按 Ctrl+C 退出"

**Climax：** 小李打开浏览器，输入 `http://design-lib.intranet`（公司设计资源库内网域名），网页加载出来了！

**Resolution：** 小李直接拖了一个 10GB 素材包下载，速度 8 MB/s，没断过。她在工作群发了个"赞"的表情。

**揭示的需求：**

- 中文部署指南，自动识别操作系统
- 客户端 .pkg/.exe/.deb 安装包（无需命令行编译）
- 强制首次登录改密
- 连接成功后明确显示虚拟 IP
- 隧道吞吐稳定（≥ 50 Mbps 实际可感知速率）

---

#### Journey 2.2：断线恢复（Edge Case）

**Opening Scene：** 小李正在远程编辑设计稿，孩子放学到家，她需要从 Wi-Fi 切到手机热点开车去接，回家后再切回 Wi-Fi。

**Rising Action：**

1. 切换网络瞬间，客户端检测到隧道断开
2. 客户端状态栏图标变成"黄色，连接中"
3. 客户端自动尝试重连，3 秒后状态变成"绿色，已连接"
4. 小李回到设计稿，发现刚才正在保存的文件没出错

**Climax：** 全程小李没操作客户端，甚至没注意到断过。

**Resolution：** 小李第二天告诉同事"这个 VPN 比我之前用的稳多了"。

**揭示的需求：**

- 客户端常驻后台 daemon
- 网络变化自动检测（监听网络接口事件）
- 指数退避自动重连
- 客户端状态可视化（图标/通知栏）

---

### Journey Requirements Summary

| 能力领域 | 旅程来源 | 关键需求 |
|---------|---------|---------|
| **快速部署** | 1.1 | Docker 单容器、自动 HTTPS、引导式 admin 初始化 |
| **账号管理** | 1.2, 1.3 | 用户 CRUD、自动生成强密码、密码重置、强制首次改密 |
| **节点监控** | 1.2, 1.3 | 实时在线状态、心跳超时检测、强制下线 |
| **审计排查** | 1.3 | 登录日志（含失败原因）、操作审计、节点连接日志 |
| **客户端体验** | 2.1, 2.2 | 跨平台安装包、CLI 简单命令、自动重连、后台 daemon |
| **隧道质量** | 2.1, 2.2 | 实际吞吐 ≥ 50 Mbps、网络切换无感、长连接稳定 |
| **管理体验** | 全部 | 中文 UI、引导式操作、零 SSH 介入 |

## Domain-Specific Requirements

### Compliance & Regulatory（合规与法规）

**国内合规要求（中小企业适用范围）：**

- **《网络安全法》(2017)** ：作为网络运营者，需采取技术措施保护用户信息
- **《个人信息保护法》(PIPL, 2021)** ：处理用户账号信息（用户名、邮箱、密码哈希）属于个人信息处理活动
- **《网络数据安全管理条例》(2025.1.1)**：
  - 用户数据需存储在境内服务器（自托管 ✓ 天然合规）
  - 登录日志保留 ≥ 6 个月
- **等保 2.0**：作为中小企业自用工具不强制，但服务对外时若被使用方要求需达二级
- **明确不涉及**：本系统是企业内网组网工具，不属于跨境数据传输/翻墙业务，无 VPN 经营许可问题

**国际参考标准（设计借鉴，非强制）：**

- **RFC 8439**：ChaCha20-Poly1305（WireGuard 数据加密所用，已是 IETF 标准）
- **OWASP ASVS 4.0**：应用安全验证标准，作为账号认证模块的设计参考
- **NIST SP 800-63B**：密码强度、认证因子最佳实践参考

### Technical Constraints（技术约束）

**密码学要求（不可妥协项）：**

- 密码存储：必须使用 argon2id（OWASP 2024 推荐）；禁止 MD5/SHA1/明文/简单加盐 SHA256
- 传输加密：管理后台 HTTPS 强制（TLS 1.2+，禁用 SSLv3/TLS 1.0/1.1）
- 隧道加密：WireGuard 默认套件（Curve25519 + ChaCha20-Poly1305 + BLAKE2s），不提供"关闭加密"选项
- 密钥生成：必须使用 OS 级 CSPRNG（`OsRng`），禁止 thread_rng 用于密钥

**身份认证要求：**

- 密码最小复杂度：≥ 8 位，含字母+数字（admin 自定义策略可调更严）
- 防爆破：登录接口限速（5 次/分钟/IP），失败 5 次锁定 15 分钟（指数退避）
- 会话管理：JWT Access Token 15 分钟过期；Refresh Token 30 天过期，支持服务端撤销
- 强制首次改密：admin 创建用户时生成的初始密码必须强制修改

**密钥与隐私保护：**

- 客户端 WireGuard 私钥**必须在客户端本地生成**，服务端永不接触
- 服务端只存储客户端公钥
- 用户账号删除时，DB 事务原子清理：账号 + WireGuard peer + 所有活跃 Token

**审计与日志：**

- 必记录事件：登录成功/失败（含 IP、UA、失败原因）、密码修改、用户增删、节点连接/断开
- 日志保留 ≥ 6 个月（合规要求）
- 日志写入失败不阻塞业务，但记录到 stderr（防止日志丢失）

**网络隔离与权限：**

- 服务端进程以非 root 用户运行（仅授予 `CAP_NET_ADMIN` capability）
- TUN 设备读写、netlink 操作必须使用最小权限模型
- 默认 systemd 沙箱：`ProtectSystem=strict`、`PrivateTmp`、`NoNewPrivileges`

### Integration Requirements（集成要求）

**操作系统集成（必需）：**

- Linux：内核 ≥ 5.6 推荐（原生 WireGuard 支持），最低 4.19；TUN/TAP `/dev/net/tun`；iptables NAT 规则
- macOS：≥ 11.0（Big Sur）；`utun` 接口；自签名/公证安装包
- Windows：≥ 10 (1809)；WinTUN 驱动；管理员权限安装

**网络环境兼容：**

- 支持服务端在 NAT 后部署（用户配置端口映射）
- 客户端在双重 NAT / CGNAT 环境下能与服务端建立连接
- 支持 IPv4 公网；IPv6 后续支持

**MVP 不集成（明确排除）：**

- LDAP / Active Directory / OAuth2 SSO（Phase 3 Vision 范围）
- 第三方监控系统的标准化集成（Prometheus 在 Phase 2 提供，非 MVP）
- 移动端应用（iOS/Android 在 Vision 范围）

### Risk Mitigations（领域特定风险与缓解）

| 风险 | 严重度 | 缓解措施 |
|------|-------|---------|
| **认证爆破**：弱密码被暴力破解 | 高 | argon2id + 限速 + 失败锁定 + 强制密码强度 |
| **WireGuard 私钥泄露**：客户端被入侵 | 高 | 私钥仅客户端本地存储；admin 可一键撤销该 peer 强制重新生成 |
| **服务端崩溃丢失配置**：导致全员断连 | 中 | DB 持久化所有 peer；服务重启时从 DB 重建 WireGuard 配置 |
| **跨平台 TUN 行为差异**：bug 难定位 | 中 | CI 矩阵测试三平台；统一抽象层（defguard_wireguard_rs） |
| **管理后台被未授权访问** | 高 | 强制 HTTPS + JWT + RBAC + 审计日志 + IP 白名单（可选） |
| **timer 未更新导致静默断线**（boringtun 已知陷阱） | 高 | 独立 tokio task 每 100ms 调用 `update_timers()`；监控告警 |
| **服务端时间漂移导致 JWT 失效** | 低 | systemd-timesyncd / chrony 强制 NTP 同步 |
| **日志泄露用户数据** | 中 | 密码/Token 在日志中脱敏；只记录用户名+IP，不记录原始密码 |

### Domain Anti-Patterns（应避免的反模式）

- **自实现密码学原语**：WireGuard 已固化算法，不允许自选加密（避免引入弱算法）
- **可选择关闭 HTTPS**：管理后台必须 HTTPS，不提供 HTTP 模式（即使是"测试用"）
- **服务端保存用户私钥**：违反零信任原则；只存公钥
- **使用 root 运行整个服务**：使用最小 capability
- **登录失败提示过于详细**：错误提示统一为"用户名或密码错误"，不区分（防用户名枚举）

## API Backend & CLI Tool Specific Requirements

### Project-Type Overview

本项目包含三个交付物：

1. **服务端二进制（API Backend）**：Axum REST + WebSocket，单进程包含 VPN 数据平面 + 控制平面
2. **客户端 CLI 工具**：跨平台命令行 + 后台 daemon，支持脚本化与交互式
3. **管理后台前端**：React SPA，与服务端 REST API 解耦（独立打包/部署）

### API Endpoint Specifications

**MVP 必需 API 端点（版本化路径 `/api/v1/`）：**

#### 认证模块

| Method | Path | 描述 | 认证 |
|--------|------|------|------|
| `POST` | `/api/v1/auth/login` | 用户名+密码登录，返回 Access + Refresh Token | 公开 |
| `POST` | `/api/v1/auth/refresh` | 用 Refresh Token 换 Access Token | Refresh Token |
| `POST` | `/api/v1/auth/logout` | 撤销当前 Refresh Token | JWT |
| `POST` | `/api/v1/auth/change-password` | 修改自己的密码 | JWT |
| `POST` | `/api/v1/auth/first-time-setup` | 首次部署创建初始 admin（一次性接口） | 公开（仅 admin 未存在时可用） |

#### Peer 节点模块

| Method | Path | 描述 | 认证 |
|--------|------|------|------|
| `POST` | `/api/v1/peers/register` | 客户端注册（提交 WireGuard 公钥），返回 VPN IP + 服务端配置 | JWT |
| `POST` | `/api/v1/peers/heartbeat` | 心跳保活，更新 endpoint | JWT |
| `GET` | `/api/v1/peers/me/config` | 下载当前节点的 WireGuard .conf 文件 | JWT |
| `DELETE` | `/api/v1/peers/me` | 客户端主动注销 | JWT |

#### 管理后台 API（需 admin 角色）

| Method | Path | 描述 |
|--------|------|------|
| `GET` | `/api/v1/admin/users` | 用户列表（分页、搜索） |
| `POST` | `/api/v1/admin/users` | 创建用户（含自动生成初始密码） |
| `PATCH` | `/api/v1/admin/users/:id` | 修改用户（重置密码、禁用启用） |
| `DELETE` | `/api/v1/admin/users/:id` | 删除用户（级联清理 peer + token） |
| `GET` | `/api/v1/admin/peers` | 所有节点列表（在线状态、虚拟 IP、最后心跳） |
| `DELETE` | `/api/v1/admin/peers/:id` | 强制下线节点 |
| `GET` | `/api/v1/admin/audit-logs` | 审计日志（登录、配置变更） |
| `GET` | `/api/v1/admin/system/info` | 服务端信息（VPN 网段、服务端公钥、端点、版本） |

#### WebSocket

| Path | 描述 | 认证 |
|------|------|------|
| `/api/v1/ws/admin/events` | 实时推送节点状态变化（PeerJoined/PeerLeft/PeerUpdated） | JWT（admin） |

#### 公开页面

| Path | 描述 |
|------|------|
| `GET /` | 管理后台前端 SPA |
| `GET /setup` | 中文部署/客户端下载引导页（公开） |
| `GET /health` | 健康检查（无认证） |
| `GET /metrics` | Prometheus metrics（Phase 2，IP 白名单） |

### Authentication Model

- **方案**：JWT（RS256 非对称签名）+ Refresh Token 轮换机制
- **Access Token**：15 分钟过期，由 `Authorization: Bearer <token>` Header 携带
- **Refresh Token**：30 天过期，httpOnly Cookie 存储；服务端在 DB/Redis 维护 Token 哈希，支持显式撤销
- **密码哈希**：argon2id（参数 m=64MB, t=3, p=2）
- **限速**：登录接口 5 次/分钟/IP（tower-governor）
- **失败锁定**：连续 5 次失败 → 锁定 15 分钟（指数退避，Redis 计数）

### Data Schemas（核心数据契约）

**统一响应信封：**

```json
{
  "code": 0,
  "message": "success",
  "data": { ... },
  "timestamp": 1715414400,
  "request_id": "uuid-v4"
}
```

**关键资源 Schema：**

```typescript
interface User {
  id: string;              // UUID
  username: string;        // 唯一
  email: string;
  role: 'admin' | 'user';
  status: 'active' | 'disabled';
  created_at: string;      // ISO 8601
  last_login_at: string | null;
}

interface Peer {
  id: string;              // UUID
  user_id: string;
  device_name: string;     // 客户端自报，如 "lin's MacBook"
  wg_public_key: string;   // Base64 编码
  vpn_ip: string;          // 如 "10.8.0.5"
  endpoint: string | null; // 公网 endpoint（IP:port）
  last_seen: string | null;
  status: 'online' | 'offline';
  os_info: string;         // 如 "macOS 14.0"
}

interface AuditLog {
  id: string;
  user_id: string | null;
  username: string;        // 冗余存储，防止用户删除后日志变空
  action: string;          // 如 'login_success', 'user_created', 'peer_registered'
  resource: string;        // 如 'user:abc-123', 'peer:def-456'
  ip_addr: string;
  user_agent: string;
  metadata: object;
  created_at: string;
}
```

### Error Codes

| 范围 | 类型 | 示例 |
|------|------|------|
| `1xxx` | 认证错误 | `1001` 用户名或密码错误、`1002` Token 过期、`1003` 账号已锁定 |
| `2xxx` | 权限错误 | `2001` 需要 admin 角色、`2002` 资源无访问权限 |
| `3xxx` | 资源错误 | `3001` 用户不存在、`3002` 节点不存在、`3003` 用户名已存在 |
| `4xxx` | 限额/限速 | `4001` 请求过频、`4002` 节点数超限 |
| `5xxx` | 系统错误 | `5001` 数据库异常、`5002` WireGuard 配置失败、`5003` 内部错误 |

### Rate Limits

| 端点 | 限额 | 说明 |
|------|------|------|
| `POST /api/v1/auth/login` | 5/分钟/IP | 防爆破 |
| `POST /api/v1/auth/refresh` | 60/分钟/IP | 正常使用频率上限 |
| `POST /api/v1/peers/heartbeat` | 6/分钟/Token | 心跳 10s 间隔的两倍冗余 |
| 其他 API | 600/分钟/IP | 通用上限，防滥用 |
| `/api/v1/ws/admin/events` | 5 并发连接/Token | 防 WebSocket 滥连 |

### API Documentation

- **MVP**：基础 README + 端点列表（Markdown）
- **Phase 2**：集成 `utoipa` 自动生成 OpenAPI 3.0 规范 + Swagger UI（`/api/docs`）
- **不做**：SDK 生成（Phase 3+ 视社区需求决定）

### CLI Tool Specifications

**命令结构：**

```
vpn-cli <command> [options]

Commands:
  login <server-url>      登录服务端（交互式输入密码）
  logout                  注销并断开
  start                   启动 VPN 隧道（如未登录则提示）
  stop                    停止 VPN 隧道
  status                  显示当前连接状态（虚拟 IP、对端、流量）
  daemon                  以前台 daemon 模式运行（systemd/launchd 用）
  config                  显示当前配置文件路径
  version                 显示版本

Options:
  --json                  以 JSON 格式输出（适合脚本化）
  --config <path>         指定配置文件路径
  --verbose, -v           详细日志输出
  --quiet, -q             仅输出错误
```

**Output Formats（双模式）：**

```bash
# 人类可读（默认）
$ vpn-cli status
✓ 已连接
  服务端：vpn.acme.com:8443
  虚拟 IP：10.8.0.5
  入站流量：12.3 MB
  出站流量：45.6 MB
  在线时长：2h 15m

# JSON 模式（脚本化）
$ vpn-cli status --json
{"connected":true,"server":"vpn.acme.com:8443","vpn_ip":"10.8.0.5","rx_bytes":12891024,"tx_bytes":47845632,"uptime_secs":8100}
```

**Config Method：**

- 配置文件：`~/.config/vpn-cli/config.toml`（Linux/macOS）、`%APPDATA%\vpn-cli\config.toml`（Windows）
- 凭证存储：
  - macOS：Keychain
  - Linux：libsecret（GNOME Keyring / KWallet）；fallback 加密文件
  - Windows：Credential Manager
- WireGuard 私钥：始终本地加密存储，永不上传

**Scripting Support：**

- 所有命令支持 `--json` 输出，便于 shell/Ansible 脚本调用
- 退出码：0 成功，1 通用错误，2 认证失败，3 网络错误，4 配置错误
- `vpn-cli daemon` 接受 SIGTERM 优雅退出

**Shell Completion（Phase 2，非 MVP 必需）：**

- bash、zsh、fish、powershell 补全脚本
- 通过 `vpn-cli completions <shell>` 生成

### Technical Architecture Considerations

**服务端二进制特性：**

- 单一可执行文件（静态链接 musl 优先，glibc 备选）
- 启动时验证：数据库连接 + WireGuard 接口创建 + TLS 证书
- 启动失败明确报错（不静默退出）
- 优雅关闭：捕获 SIGTERM，等待当前请求完成（最长 30s），关闭 WireGuard 接口

**客户端二进制特性：**

- 单一可执行文件，无运行时依赖
- macOS：.pkg 安装包（自动安装 LaunchAgent）
- Linux：.deb / .rpm（自动安装 systemd 用户服务）；通用 tar.gz
- Windows：.msi 安装包（自动注册 Windows Service）
- 守护进程：CLI 命令通过 Unix Domain Socket / Named Pipe 与 daemon 通信

### Implementation Considerations

**MVP 不实现（明确范围排除）：**

- GraphQL 接口（仅 REST + WebSocket）
- gRPC 接口（无内部服务间调用）
- SDK 自动生成（Go/Python/JS SDK）
- Webhook 回调系统
- 第三方 IDP 集成（LDAP/OAuth2/SAML）
- 多租户隔离（单租户场景）
- API 版本协商（仅 v1，未来 v2 通过路径区分）

## Project Scoping & Phased Development

### MVP Strategy & Philosophy

**MVP 类型：体验式 MVP（Experience MVP）**

不是"问题解决式 MVP"（仅证明协议可通），也不是"平台式 MVP"（建立扩展性），而是**让一位真实 IT 管理员能完整跑通"部署 → 添加用户 → 员工连接 → 日常运维"全流程的最小完整体验**。

**理由：**

- 项目核心差异化是"易用性"，不是新协议或新架构
- 验证假设的最快路径是让真实用户使用，而非技术 demo
- 同类竞品（innernet/NetBird/Tailscale）已证明技术可行性，无需重复验证

**MVP 验收标准（关联第 3 步成功指标）：**

- 5 位无 VPN 经验者中 ≥ 4 位能在 30 分钟内完成首次部署 + 客户端连接
- 一家真实小企业（5–10 人）稳定使用 ≥ 30 天
- 客户端断线自动重连成功率 ≥ 99%

**资源需求：**

- 团队：1–2 名 Rust 开发者（兼前端）+ 1 名兼职 IT 管理员协助实测
- 时间：3–5 周 MVP，4–6 周 Growth，3–5 个月达到生产可用

### MVP Feature Set（Phase 1，3–5 周）

> 注：详细功能列表已在前面 "Product Scope - MVP" 章节定义，本节仅强调 MVP 边界决策。

**核心 MVP 必含（Must-Have）：**

| 决策 | 理由 |
|------|------|
| Hub-and-Spoke 拓扑（不做 Mesh） | Mesh + NAT 穿透实现复杂度是 Hub-Spoke 的 3–4 倍；MVP 阶段不验证此差异化 |
| 单机部署（不做高可用集群） | 中小企业 < 200 节点场景下单机足够；HA 不在 MVP 价值假设中 |
| 后台管理 UI（必含） | 用户明确确认 MVP 必须含 UI，否则差异化定位（vs innernet）无法成立 |
| 跨平台 CLI 客户端三平台 | MVP 用户旅程明确涉及 macOS（小李），跨平台是基础要求 |
| SQLite（不上 PostgreSQL） | 小规模场景 SQLite 足够，零外部依赖，符合"5 分钟部署"承诺 |
| 中文 UI + 中文文档 | 核心差异化定位 |

**MVP 明确排除（Deferred to Growth/Vision）：**

| 排除项 | 推迟原因 |
|-------|---------|
| Mesh P2P 直连 | 复杂度高，且小型场景 Hub-Spoke 性能已足 → Vision |
| NAT 穿透（UDP 打洞） | 中小企业服务端通常有公网 IP → Vision |
| 流量统计 | 非阻塞核心使用，添加复杂度 → Growth |
| Prometheus/Grafana 监控 | 50 节点以下查看日志即可定位问题 → Growth |
| 客户端 GUI（Tauri） | CLI + 后台 UI 已能覆盖核心场景 → Growth |
| RBAC 多角色 | MVP 仅 admin + user 二元角色 → Growth 引入 Operator |
| LDAP/SSO | 中小企业账号体量小，本地账号足够 → Vision |
| iOS/Android | MVP 用户旅程聚焦桌面办公场景 → Vision |
| 多语言（i18n 英文） | 核心差异化是中文优先，英文是未来扩展 → Vision |

### Post-MVP Features

**Phase 2 - Growth Features（Post-MVP，4–6 周）：**

详见前面 "Product Scope - Growth Features"。聚焦于：

- 可观测性（监控、流量、审计日志）
- 易用性增强（WebSocket 实时推送、Tauri GUI）
- 规模扩展（PostgreSQL）
- 权限精细化（RBAC）

**Phase 3 - Vision（中长期）：**

详见前面 "Product Scope - Vision"。聚焦于：

- 拓扑升级（Mesh）
- 高可用（HA）
- 企业级集成（SSO、路由策略）
- 移动端

### Risk Mitigation Strategy

**技术风险：**

| 风险 | 缓解策略 |
|------|---------|
| boringtun timer 陷阱导致静默断线 | 优先开发 timer 子模块，独立测试覆盖；监控 `last_handshake` 指标 |
| 跨平台 TUN 差异（macOS utun / Windows WinTUN） | 使用 `defguard_wireguard_rs` 统一抽象；CI 矩阵测试三平台；先实现 Linux 验证主流程，macOS/Windows 在 Sprint 后期 |
| Rust 异步死锁（std::sync::Mutex 跨 await） | clippy 强制检查；架构层规定全部使用 tokio::sync；代码审查双人 |

**市场风险：**

| 风险 | 缓解策略 |
|------|---------|
| 用户最终选择 NetBird 自托管 | 早期定位非"全面替代"，而是"中文 + 轻量"细分市场；与 NetBird 共存 |
| 中小企业实际付费意愿低（开源项目难变现） | 不预设商业化，MVP 阶段定位"开源工具 + 个人 IT 主管痛点工具" |
| 等保/合规需求被低估 | MVP 明确说明"非等保认证产品"；为后续 Growth 阶段预留合规扩展点 |

**资源风险：**

| 风险 | 缓解策略 |
|------|---------|
| 单人开发难以同时覆盖 Rust + 前端 | 前端选 Ant Design Pro（开箱模板）+ AI 辅助；可外包 UI 给社区贡献者 |
| MVP 周期超期（3–5 周变 8 周） | 严格控制 MVP 排除项，不接受范围蠕变；周度自我评估 |
| 测试覆盖不足导致质量风险 | MVP 阶段强制要求：单元测试覆盖核心模块；E2E 测试覆盖三个核心旅程 |

## Functional Requirements

> **能力契约说明：** 本节定义产品必须具备的所有能力。每条 FR 描述 WHO 能 WHAT，不规定 HOW。MVP 必含项标 [MVP]，Growth 标 [G]，Vision 标 [V]。

### A. 部署与初始化（Deployment & Initialization）

- **FR1** [MVP] 任何人可以通过单条容器运行命令完成服务端部署（不依赖外部数据库/缓存）
- **FR2** [MVP] 系统能在首次启动时自动申请并配置 HTTPS 证书（基于域名）
- **FR3** [MVP] 系统能在首次访问 Web 后台时引导首位访问者创建初始 admin 账号
- **FR4** [MVP] admin 可以在后台查看服务端配置信息（VPN 网段、服务端公钥、监听端点、版本号）
- **FR5** [MVP] 系统在重启后能自动从持久化存储恢复所有节点的 WireGuard 配置，无需 admin 介入

### B. 账号与认证（Account & Authentication）

- **FR6** [MVP] 用户可以用账号+密码登录系统（通过 CLI 或 Web 后台）
- **FR7** [MVP] 用户可以修改自己的密码
- **FR8** [MVP] 用户首次登录使用初始密码时，系统强制要求修改为自定义密码后才能继续操作
- **FR9** [MVP] 已登录用户可以主动注销当前会话
- **FR10** [MVP] 系统能在用户连续多次登录失败后临时锁定该账号
- **FR11** [MVP] 系统会在登录失败时返回统一的错误信息（不区分用户名错误与密码错误）
- **FR12** [MVP] 系统能区分 admin 与普通 user 两种角色，并对应不同的可访问操作
- **FR13** [G] 系统支持 RBAC 三角色（Admin / Operator / User），可配置更细粒度的权限

### C. 用户管理（User Management - admin）

- **FR14** [MVP] admin 可以创建新用户（指定用户名、邮箱，由系统自动生成强初始密码）
- **FR15** [MVP] admin 可以查看所有用户列表（含状态、创建时间、最后登录时间）
- **FR16** [MVP] admin 可以分页与按用户名/邮箱搜索用户列表
- **FR17** [MVP] admin 可以重置任意用户的密码（重置后用户下次登录需强制改密）
- **FR18** [MVP] admin 可以禁用/启用用户账号（禁用账号无法登录但保留数据）
- **FR19** [MVP] admin 可以删除用户账号（级联清理其所有节点配置与活跃 Token）
- **FR20** [MVP] admin 可以复制系统生成的"员工接入指南"链接（包含客户端下载、登录步骤）

### D. 节点（Peer）管理与连接

- **FR21** [MVP] 已登录用户可以从其客户端注册一个新节点（提交本地生成的 WireGuard 公钥）
- **FR22** [MVP] 系统能为新注册的节点自动从 VPN 网段中分配唯一虚拟 IP
- **FR23** [MVP] 系统能为已注册节点保持稳定的虚拟 IP 绑定（重连/重启不更换 IP）
- **FR24** [MVP] 注册成功的节点可以下载其完整 WireGuard 配置（含服务端公钥、虚拟 IP、网段、端点）
- **FR25** [MVP] 已连接节点可以定期发送心跳，系统据此更新节点的在线状态与公网 endpoint
- **FR26** [MVP] 用户可以主动注销自己的节点（断开隧道并从服务端移除 peer）
- **FR27** [MVP] admin 可以查看所有节点列表（含所属用户、虚拟 IP、在线状态、最后心跳时间、公网 endpoint、操作系统）
- **FR28** [MVP] admin 可以强制下线任意节点（立即从服务端 WireGuard 配置移除）
- **FR29** [MVP] 系统能在节点连续未发送心跳超过阈值后自动标记为离线状态

### E. 客户端体验（Client Experience）

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

### F. 隧道运维（Tunnel Operations）

- **FR41** [MVP] 系统能在客户端与服务端之间建立加密 VPN 隧道（基于 WireGuard 协议）
- **FR42** [MVP] 已连接节点间可以通过虚拟 IP 互相通信（经服务端 Hub-Spoke 转发）
- **FR43** [MVP] 客户端能在网络断开或服务端不可达时自动尝试重连（指数退避）
- **FR44** [MVP] 客户端能在本机网络环境变化时（Wi-Fi 切换等）自动检测并重建隧道
- **FR45** [MVP] 客户端能向用户显示当前连接状态变化（连接中 / 已连接 / 已断开）
- **FR46** [V] 节点之间可以通过 P2P 直连方式通信（绕过服务端转发）
- **FR47** [V] 客户端能在双重 NAT/CGNAT 环境下与对端建立 P2P 直连（UDP 打洞）

### G. 审计与可见性（Audit & Visibility）

- **FR48** [MVP] 系统能记录所有登录尝试（含用户名、IP、UA、成功/失败、失败原因）
- **FR49** [MVP] 系统能记录所有配置变更操作（用户增删、密码重置、节点强制下线等）
- **FR50** [MVP] 系统能记录所有节点连接/断开事件
- **FR51** [MVP] admin 可以在后台查询审计日志（含分页、按时间/用户/操作类型过滤）
- **FR52** [MVP] 系统能将审计日志保留至少 6 个月
- **FR53** [G] admin 可以查看每节点 / 每用户的流量统计（入站/出站字节数、时间段聚合）
- **FR54** [G] admin 可以通过 WebSocket 实时接收节点状态变化推送（无需轮询）
- **FR55** [G] 系统能以 Prometheus metrics 格式暴露关键指标（在线节点数、流量、认证失败率）

### H. 安全防护（Security Protection）

- **FR56** [MVP] 系统能对登录端点做请求限速（防爆破）
- **FR57** [MVP] 系统能在认证失败超过阈值后临时锁定账号（指数退避）
- **FR58** [MVP] 系统能使用 argon2id 算法存储用户密码
- **FR59** [MVP] 系统能强制使用 HTTPS 访问管理后台（拒绝 HTTP 请求）
- **FR60** [MVP] 系统能在用户被删除时原子清理其所有相关数据（peer 配置 + Token + 凭据）
- **FR61** [MVP] 系统能验证创建/修改密码时的密码强度（最小长度、字符种类）
- **FR62** [G] admin 可以配置管理后台访问的 IP 白名单（限制后台访问来源）
- **FR63** [V] admin 可以将系统接入企业 IDP（LDAP / OAuth2 / SAML）进行单点登录

---

**总计：63 项功能需求（MVP: 49 项 / Growth: 8 项 / Vision: 6 项）**

**能力契约提醒：** 此列表一旦确认即为产品最终能力的范围基线。任何未列出的能力都不会出现在最终产品中（除非显式补充）。

## Non-Functional Requirements

> 每条 NFR 都是可测量、可验证的质量约束。MVP 必达项标 [MVP]，Growth 提升目标标 [G]。

### Performance（性能）

- **NFR-P1** [MVP] 单 VPN 隧道在千兆带宽下吞吐 ≥ 100 Mbps（boringtun userspace 实测）
- **NFR-P2** [MVP] 50 个并发节点场景下，服务端 CPU 占用 < 30%（4 vCPU 主机）
- **NFR-P3** [MVP] 50 个并发节点场景下，服务端常驻内存 < 1 GB
- **NFR-P4** [MVP] 客户端从 `vpn-cli start` 到隧道建立完成 ≤ 10 秒
- **NFR-P5** [MVP] 管理后台 API（用户列表、节点列表）p95 响应时间 ≤ 300 ms（50 节点数据量）
- **NFR-P6** [MVP] 客户端心跳间隔 = 30 秒；WireGuard PersistentKeepalive = 25 秒
- **NFR-P7** [G] 单 VPN 隧道吞吐 ≥ 500 Mbps（性能优化目标）

### Security（安全）

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

### Reliability（可靠性）

- **NFR-R1** [MVP] 服务端连续运行 ≥ 30 天不需重启（验证内存泄漏与稳定性）
- **NFR-R2** [MVP] 月度可用性 ≥ 99.5%（每月不可用时间 ≤ 3.6 小时）
- **NFR-R3** [MVP] 客户端断线后自动重连成功率 ≥ 99%（网络抖动、服务端重启场景）
- **NFR-R4** [MVP] 服务端进程异常退出后能由 systemd/Docker 自动重启
- **NFR-R5** [MVP] 服务端重启后能在 30 秒内从持久化存储恢复所有 WireGuard peer 配置
- **NFR-R6** [MVP] 数据库写入失败时不阻塞客户端心跳响应（降级写本地缓存，后台重试）
- **NFR-R7** [MVP] WireGuard timer 必须每 ≤ 250ms 调用一次 `update_timers()`（防静默断线）

### Scalability（可扩展性）

- **NFR-SC1** [MVP] 单服务端实例稳定支持 ≥ 50 并发活跃节点
- **NFR-SC2** [MVP] 单服务端实例稳定支持 ≥ 500 注册用户账号（离线节点占多数）
- **NFR-SC3** [G] 单服务端实例支持 ≥ 200 并发活跃节点（性能优化目标）
- **NFR-SC4** [G] 数据库存储层可平滑从 SQLite 切换到 PostgreSQL（无业务逻辑变更）

### Compatibility（兼容性）

- **NFR-C1** [MVP] 服务端在 Linux x86_64 内核 ≥ 4.19 上运行（推荐 ≥ 5.6）
- **NFR-C2** [MVP] 客户端在 Linux x86_64/aarch64（Ubuntu 20.04+/Debian 11+/CentOS 8+）运行
- **NFR-C3** [MVP] 客户端在 macOS ≥ 11.0（Big Sur）x86_64/aarch64 运行
- **NFR-C4** [MVP] 客户端在 Windows ≥ 10 (1809) x86_64 运行
- **NFR-C5** [MVP] 三平台客户端功能完全对等（同样 CLI 命令、行为）
- **NFR-C6** [MVP] 管理后台前端兼容 Chrome/Edge/Safari/Firefox 最近 2 个主版本

### Operability（可运维性）

- **NFR-O1** [MVP] 服务端从首次容器启动到管理后台可访问 ≤ 5 分钟（含自动 HTTPS 证书申请）
- **NFR-O2** [MVP] 一次完整部署所需手动操作 ≤ 3 步（拉取镜像 / 启动容器 / 首次访问 setup）
- **NFR-O3** [MVP] 部署完成后无需 SSH 进入服务器即可完成全部日常运维（账号、节点、日志）
- **NFR-O4** [MVP] 客户端安装到首次连接 ≤ 3 分钟（按引导操作）
- **NFR-O5** [MVP] 服务端启动失败时通过 stderr 输出明确错误（数据库不可达 / 端口被占 / 证书失败）
- **NFR-O6** [MVP] 服务端配置变更（添加用户等）即时生效，无需重启服务
- **NFR-O7** [MVP] README 文档完整覆盖部署、使用、故障排查（中文，附截图）
- **NFR-O8** [G] 服务端能在 systemd-journald 输出结构化 JSON 日志（便于采集分析）

### Maintainability（可维护性，开发侧）

- **NFR-M1** [MVP] 核心业务模块单元测试覆盖率 ≥ 70%（认证、Peer 管理、WireGuard 集成）
- **NFR-M2** [MVP] 提供至少 3 个端到端集成测试覆盖核心用户旅程（部署、新员工接入、断线重连）
- **NFR-M3** [MVP] CI 在每次 PR 提交时运行 `cargo fmt --check` + `cargo clippy -D warnings` + `cargo test`
- **NFR-M4** [MVP] CI 矩阵测试 Linux + macOS + Windows 三平台编译通过
- **NFR-M5** [MVP] 所有公开 API 端点提供 OpenAPI 文档（Markdown 形式）

---

**NFR 总计：43 项（MVP: 38 项 / Growth: 5 项）**
