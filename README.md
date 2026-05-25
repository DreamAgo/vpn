# vpn

> 面向中小型企业（20–200 人）的轻量级自托管异地组网 VPN。基于 WireGuard，账号密码登录 + Web 管理后台，30 分钟内完成部署并接入第一个客户端。

[![CI](https://github.com/OWNER/vpn/actions/workflows/ci.yml/badge.svg)](../../actions/workflows/ci.yml)

## 这是什么

`vpn` 让你用一条命令在自己的服务器上跑起一个企业 VPN：

- **服务端**单容器部署，自动申请 HTTPS 证书，内置 Web 管理后台。
- **管理员**在后台用账号密码登录，创建/禁用/删除员工账号，查看节点在线状态与审计日志。
- **员工**用跨平台命令行客户端（macOS / Linux / Windows）账号密码登录后一键连接，获得稳定的虚拟 IP，实现异地组网与节点互通。
- 数据平面基于 **WireGuard**（boringtun 用户态实现），控制平面用 Rust + Axum，前端 React + Ant Design Pro。

> **项目状态**：核心功能（认证、用户管理、节点数据平面、监控审计、CLI 客户端）已实现并通过测试；真实 WireGuard 隧道转发与三平台系统集成需在目标平台真机验证，详见 [`docs/REAL-HARDWARE-CHECKLIST.md`](docs/REAL-HARDWARE-CHECKLIST.md)。

## 架构一览

```
                    ┌─────────────────────────────────────┐
   管理员浏览器 ───▶ │  vpn-server (单进程 / 单容器)         │
   (HTTPS Web UI)   │  ┌─────────────┬──────────────────┐  │
                    │  │ Axum HTTP   │ React 后台(内嵌)  │  │
                    │  │ + JWT 认证  │ rust-embed        │  │
                    │  ├─────────────┴──────────────────┤  │
   员工客户端 ─────▶ │  │ 用户/会话/节点/审计  (SQLite)   │  │
   (vpn-cli)        │  ├────────────────────────────────┤  │
       │            │  │ WireGuardControl + IP 池        │  │
       │  WireGuard │  │ (boringtun 用户态, UDP 51820)   │  │
       └───UDP──────┼─▶└────────────────────────────────┘  │
                    └─────────────────────────────────────┘
```

详见 [`docs/architecture.md`](docs/architecture.md) 与规划文档 [`_bmad-output/planning-artifacts/architecture.md`](_bmad-output/planning-artifacts/architecture.md)。

## 技术栈

| 层 | 选型 |
|---|---|
| 后端 | Rust · tokio · axum 0.8 · sqlx(SQLite) · argon2id · JWT(RS256) |
| 数据平面 | WireGuard（boringtun 用户态）· x25519 密钥 · 静态 IP 池 |
| 客户端 | Rust · clap · tun-rs（跨平台 TUN）· keyring（系统凭据库）· Unix Socket / Named Pipe IPC |
| 前端 | React 19 · TypeScript · Vite · Ant Design 5 + Pro Components · React Query · Zustand |
| 部署 | Docker 单容器 · 自动 HTTPS（rustls-acme）· GitHub Actions 三平台 CI |

## 快速开始

### Docker 部署（推荐）

```bash
# 1. 准备环境变量
cp docker/.env.example .env   # 编辑 VPN_DOMAIN 等

# 2. 启动
docker compose -f docker/docker-compose.yml up -d

# 3. 浏览器打开 https://<你的域名>，按首次配置向导创建管理员
```

详细步骤、端口与防火墙说明见 [`docs/deployment.md`](docs/deployment.md)。

### 客户端接入

```bash
vpn-cli login --server https://vpn.example.com   # 账号密码登录
vpn-cli up                                        # 连接
vpn-cli status                                    # 查看状态
vpn-cli daemon install                            # （可选）注册为开机自启服务
```

完整客户端用法见 [`docs/client.md`](docs/client.md)。

## 文档

| 文档 | 内容 |
|---|---|
| [docs/deployment.md](docs/deployment.md) | 服务端部署（Docker / 二进制）、HTTPS、防火墙 |
| [docs/configuration.md](docs/configuration.md) | 全部环境变量参考 |
| [docs/client.md](docs/client.md) | 客户端安装与命令 |
| [docs/admin-guide.md](docs/admin-guide.md) | 后台管理：用户、节点、审计 |
| [docs/architecture.md](docs/architecture.md) | 系统架构与模块划分 |
| [docs/REAL-HARDWARE-CHECKLIST.md](docs/REAL-HARDWARE-CHECKLIST.md) | 真机收尾清单（WireGuard 数据面 / 打包 / E2E）|

## 本地开发

需安装 [Rust](https://rustup.rs/)（见 `rust-toolchain.toml`）、Node.js 20+、[just](https://github.com/casey/just)。

```bash
just            # 列出所有命令
just dev-server # 启动后端（http://localhost:8080）
just dev-frontend
just test       # 全 workspace 测试 + 前端
just lint       # fmt + clippy -D warnings + eslint
just build      # release 二进制 + 前端生产构建
```

工作区结构：

```
crates/
  vpn-api-types   前后端共享 DTO（无 IO 依赖）
  vpn-core        领域错误与 trait
  vpn-wireguard   密钥 / IP 池 / WireGuardControl 抽象
  vpn-platform    客户端跨平台：TUN / 凭据 / Daemon
  vpn-server      HTTP 服务 + 内嵌前端 + 数据平面装配
  vpn-cli         客户端命令行 + daemon
frontend/         React 管理后台
migrations/       SQLite schema
docker/           Dockerfile + compose
packaging/        各平台安装包脚手架
```

## 安全

- 密码 argon2id 哈希；登录失败指数退避锁定。
- Access Token（JWT RS256，15 分钟）+ 不透明 Refresh Token（数据库可撤销）。
- 所有写操作经审计中间件记录；审计日志按保留期自动清理。
- 依赖漏洞扫描见 `deny.toml`（`cargo deny check`）。

报告安全问题请见 [SECURITY.md](SECURITY.md)。

## License

MIT
