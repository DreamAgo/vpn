# Story 1.1: 初始化 Rust workspace 与前端项目

Status: in-progress

## Story

As a **项目开发者**,
I want **一个标准化的 Rust workspace + Vite React-TS 前端骨架**,
so that **后续所有 Epic 都有清晰的代码组织起点，并强制版本与命名约定不被违反**.

## Acceptance Criteria

**AC1：项目骨架初始化完成**

- **Given** 干净的工作目录
- **When** 按架构文档执行初始化（`cargo new --workspace` + 6 个子 crate + `npm create vite`）
- **Then** 项目根包含：`Cargo.toml`（workspace 配置含 `[workspace.dependencies]`）、`rust-toolchain.toml`（pin 到 stable 1.82+）、`Justfile`、`.gitignore`、`.editorconfig`、`README.md` 占位、`LICENSE`（MIT）
- **And** `crates/` 下存在 6 个子 crate：`vpn-api-types`、`vpn-core`、`vpn-wireguard`、`vpn-platform`、`vpn-server`、`vpn-cli`
- **And** `frontend/` 下存在 Vite + React 18 + TypeScript（strict 模式）项目骨架

**AC2：项目可构建且开发命令可用**

- **Given** 已完成项目初始化
- **When** 执行 `cargo build --workspace` 与 `cd frontend && npm install && npm run build`
- **Then** 后端编译成功（仅占位 `fn main() { println!("vpn-server placeholder") }` 等）；前端构建成功（生成 `frontend/dist/`）
- **And** `just --list` 显示至少 4 个开发命令：`dev` / `test` / `build` / `docker`（后两个可为占位）

## Tasks / Subtasks

- [ ] **Task 1：初始化 Rust workspace 根目录（AC1）**
  - [ ] 1.1 在仓库根执行 `cargo new --vcs git .` 创建初始 Cargo.toml 与 src/main.rs
  - [ ] 1.2 改写根 Cargo.toml 为 workspace 配置（参见 §Dev Notes "Workspace Cargo.toml 模板"）
  - [ ] 1.3 删除根 src/ 目录（workspace 根不含 binary）
  - [ ] 1.4 创建 `rust-toolchain.toml` 指定 `channel = "1.82"`（不用 nightly）
  - [ ] 1.5 创建 `.gitignore`（含 `/target`、`/frontend/dist`、`/frontend/node_modules`、`.env*`、`*.db`、`.sqlx/dummy`、`.DS_Store`）
  - [ ] 1.6 创建 `.editorconfig`（utf-8 + LF + 4 空格 Rust / 2 空格 TS/JSON/YAML）
  - [ ] 1.7 创建 `LICENSE` 文件（MIT）
  - [ ] 1.8 创建 `README.md` 占位（含项目名、一句话说明、"开发中"提示）

- [ ] **Task 2：创建 6 个子 crate（AC1）**
  - [ ] 2.1 `cargo new --bin crates/vpn-server`
  - [ ] 2.2 `cargo new --bin crates/vpn-cli`
  - [ ] 2.3 `cargo new --lib crates/vpn-core`
  - [ ] 2.4 `cargo new --lib crates/vpn-wireguard`
  - [ ] 2.5 `cargo new --lib crates/vpn-platform`
  - [ ] 2.6 `cargo new --lib crates/vpn-api-types`
  - [ ] 2.7 在每个 crate 的 Cargo.toml 顶部加 `edition = "2021"`、`license = "MIT"`、`publish = false`（防止误发布到 crates.io）
  - [ ] 2.8 删除每个 `cargo new` 生成的 git 子目录（仅根目录保留 git）

- [ ] **Task 3：初始化前端项目（AC1）**
  - [ ] 3.1 在仓库根执行 `npm create vite@latest frontend -- --template react-ts`（按提示，全部默认）
  - [ ] 3.2 `cd frontend && npm install`
  - [ ] 3.3 在 `frontend/tsconfig.json` 启用 `"strict": true`（Vite 默认已开，验证即可）
  - [ ] 3.4 在 `frontend/.gitignore` 已含 `node_modules/` 与 `dist/`（Vite 默认），验证
  - [ ] 3.5 修改 `frontend/index.html` `<title>` 为 "vpn 管理后台"
  - [ ] 3.6 修改 `frontend/src/App.tsx` 占位文本为 "vpn 项目正在运行"（中文，无 Vite 默认 logo 与计数器示例，删除）

- [ ] **Task 4：创建 Justfile 开发命令入口（AC2）**
  - [ ] 4.1 创建项目根 `Justfile`（参见 §Dev Notes "Justfile 模板"）
  - [ ] 4.2 包含命令：`default`（list）、`dev-server`、`dev-frontend`、`dev`（并发启动）、`test`、`build`、`docker`（占位 echo "TODO: implemented in Story 1.9"）
  - [ ] 4.3 验证 `just --list` 显示所有命令

- [ ] **Task 5：构建验证（AC2）**
  - [ ] 5.1 执行 `cargo build --workspace` 成功（占位代码编译通过）
  - [ ] 5.2 执行 `cd frontend && npm run build` 成功（产出 frontend/dist/index.html + assets）
  - [ ] 5.3 执行 `just --list` 显示命令清单
  - [ ] 5.4 提交首个 git commit："chore: initialize project workspace and frontend skeleton"

## Dev Notes

### 关键架构约束（必读）

> 来源：`_bmad-output/planning-artifacts/architecture.md` §Project Structure & Boundaries

**这是 Story 1.1，是整个项目代码的起点。后续所有 Story 都会基于本 Story 产出的 workspace。任何偏离本 Story 的目录/命名/版本决策都会被 CI 强制工具栈（rustfmt / clippy / ESLint / tsc）阻止，因此 Story 1.1 的精确性至关重要。**

### 完整项目根目录结构（本 Story 必须创建）

```
vpn/                            # 仓库根
├── README.md                   # 占位（详细文档在 Story 6.1 编写）
├── LICENSE                     # MIT
├── Cargo.toml                  # Workspace 根
├── Cargo.lock                  # cargo 自动生成
├── rust-toolchain.toml         # Pin 到 stable 1.82+
├── .gitignore
├── .editorconfig
├── Justfile                    # 统一开发命令
├── crates/
│   ├── vpn-api-types/         # lib，前后端共享 DTO（最底层）
│   ├── vpn-core/              # lib，Domain + Trait（无 IO）
│   ├── vpn-wireguard/         # lib，WireGuard 封装
│   ├── vpn-platform/          # lib，客户端跨平台抽象
│   ├── vpn-server/            # bin，服务端二进制
│   └── vpn-cli/               # bin，客户端二进制
└── frontend/                   # Vite + React 18 + TS
    ├── package.json
    ├── package-lock.json
    ├── tsconfig.json
    ├── vite.config.ts
    ├── index.html
    └── src/
        ├── main.tsx
        └── App.tsx
```

> **本 Story 仅创建骨架**。子目录下的 src/lib.rs / src/main.rs 内仅含占位代码（`fn main()` / `pub fn placeholder()`）。完整模块拆分（如 vpn-server 下的 handlers/services/repositories 目录）由后续 Story 按需创建。

### Workspace Cargo.toml 模板（精确写入，不要修改版本号）

```toml
[workspace]
resolver = "2"
members = [
    "crates/vpn-api-types",
    "crates/vpn-core",
    "crates/vpn-wireguard",
    "crates/vpn-platform",
    "crates/vpn-server",
    "crates/vpn-cli",
]

[workspace.package]
version = "0.1.0"
edition = "2021"
license = "MIT"
authors = ["Shangguanjunjie"]
repository = "https://github.com/<your-org>/vpn"
rust-version = "1.82"

[workspace.dependencies]
# === 异步运行时 ===
tokio = { version = "1.52", features = ["full"] }

# === 内部 crate ===
vpn-api-types = { path = "crates/vpn-api-types" }
vpn-core = { path = "crates/vpn-core" }
vpn-wireguard = { path = "crates/vpn-wireguard" }
vpn-platform = { path = "crates/vpn-platform" }

# === 序列化 ===
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"

# === 错误处理 ===
anyhow = "1.0"
thiserror = "2.0"

# === 日志 ===
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }

# === 时间与 ID ===
chrono = { version = "0.4", default-features = false, features = ["serde", "clock"] }
uuid = { version = "1", features = ["v7", "serde"] }

# === 异步辅助 ===
async-trait = "0.1"
futures = "0.3"

[profile.release]
opt-level = 3
lto = "thin"
codegen-units = 1
strip = true

[profile.dev]
opt-level = 0
debug = true
```

> **注：** 不要在本 Story 引入 axum / sqlx / boringtun / argon2 等具体业务依赖，这些由后续 Story 按需添加到对应 crate 的 Cargo.toml。`[workspace.dependencies]` 仅声明跨多个 crate 共享的依赖。

### 各子 crate 的 Cargo.toml 起手模板

**crates/vpn-api-types/Cargo.toml：**
```toml
[package]
name = "vpn-api-types"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
publish = false

[dependencies]
serde = { workspace = true }
serde_json = { workspace = true }
```

**crates/vpn-core/Cargo.toml：**
```toml
[package]
name = "vpn-core"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
publish = false

[dependencies]
vpn-api-types = { workspace = true }
serde = { workspace = true }
thiserror = { workspace = true }
async-trait = { workspace = true }
uuid = { workspace = true }
chrono = { workspace = true }
```

**crates/vpn-wireguard、vpn-platform：** 类似 vpn-core，仅含 `[package]` 段，无依赖（依赖由后续 Story 添加）。

**crates/vpn-server/Cargo.toml：**
```toml
[package]
name = "vpn-server"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
publish = false

[dependencies]
vpn-api-types = { workspace = true }
vpn-core = { workspace = true }
vpn-wireguard = { workspace = true }
tokio = { workspace = true }
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
anyhow = { workspace = true }
```

**crates/vpn-cli/Cargo.toml：**
```toml
[package]
name = "vpn-cli"
version.workspace = true
edition.workspace = true
license.workspace = true
authors.workspace = true
publish = false

[dependencies]
vpn-api-types = { workspace = true }
vpn-core = { workspace = true }
vpn-platform = { workspace = true }
tokio = { workspace = true }
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
anyhow = { workspace = true }
```

### 占位代码示例

**crates/vpn-server/src/main.rs：**
```rust
fn main() {
    println!("vpn-server placeholder (Story 1.5 will implement axum server)");
}
```

**crates/vpn-cli/src/main.rs：**
```rust
fn main() {
    println!("vpn-cli placeholder (Story 4.13 will implement login command)");
}
```

**crates/vpn-api-types/src/lib.rs**（类似其他 lib crate）：
```rust
//! vpn-api-types: 前后端共享 DTO（Story 1.3 起填充内容）
```

### rust-toolchain.toml

```toml
[toolchain]
channel = "1.82"
components = ["rustfmt", "clippy"]
profile = "minimal"
```

### .gitignore（项目根，附加 frontend 已有的）

```gitignore
# Rust
/target
**/*.rs.bk
Cargo.lock.bak

# 数据库与运行时
*.db
*.db-journal
.sqlx/dummy

# 环境变量
.env
.env.local
.env.*.local

# 编辑器
.idea/
.vscode/
*.swp

# 系统
.DS_Store
Thumbs.db

# 前端
/frontend/node_modules
/frontend/dist

# 私钥与证书（仅开发期）
*.pem
*.key
```

### .editorconfig

```ini
root = true

[*]
charset = utf-8
end_of_line = lf
insert_final_newline = true
trim_trailing_whitespace = true

[*.rs]
indent_style = space
indent_size = 4

[*.{ts,tsx,js,jsx,json,yaml,yml,toml,md}]
indent_style = space
indent_size = 2

[Makefile]
indent_style = tab
```

### Justfile 模板

```just
# vpn 项目开发命令入口
# 安装 just：https://github.com/casey/just

# 默认显示命令列表
default:
    @just --list

# 开发：仅启动后端（含 cargo watch 自动重载，需安装 cargo-watch）
dev-server:
    cargo watch -x 'run --bin vpn-server'

# 开发：仅启动前端（Vite dev server）
dev-frontend:
    cd frontend && npm run dev

# 开发：同时启动前端 + 后端（需安装 GNU parallel 或 concurrently）
# MVP 阶段开发者通常分两个终端跑 dev-server / dev-frontend
dev:
    @echo "请在两个终端分别运行 'just dev-server' 与 'just dev-frontend'"

# 测试：全 workspace 单元测试 + 前端测试
test:
    cargo test --workspace
    cd frontend && npm test --if-present

# 构建 release 二进制 + 前端生产构建
build:
    cd frontend && npm run build
    cargo build --release --workspace

# 构建 Docker 镜像（占位，由 Story 1.9 实现）
docker:
    @echo "TODO: Docker build will be implemented in Story 1.9"

# 代码格式与 Lint（CI 强制执行）
lint:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings
    cd frontend && npm run lint --if-present
```

### frontend/src/App.tsx 占位

```tsx
function App() {
  return (
    <div style={{
      display: 'flex',
      alignItems: 'center',
      justifyContent: 'center',
      height: '100vh',
      fontFamily: '-apple-system, BlinkMacSystemFont, "PingFang SC", "Microsoft YaHei", sans-serif',
    }}>
      <div>
        <h1>vpn 项目正在运行</h1>
        <p>Story 1.7 将配置 AntD 主题与 ProLayout 主框架</p>
      </div>
    </div>
  );
}

export default App;
```

> **注：** Story 1.1 不引入 antd / @ant-design/pro-components / react-query / zustand 等业务依赖。这些由 Story 1.7 添加。

### frontend/src/main.tsx 占位

```tsx
import React from 'react';
import ReactDOM from 'react-dom/client';
import App from './App';

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);
```

> 删除 Vite 默认的 `import './index.css'` 与示例样式文件（保持骨架最小化）。

### Testing Standards

**本 Story 不需要写单元测试**（仅项目骨架），但需保证：

- `cargo build --workspace` 成功（占位代码可编译）
- `cargo test --workspace` 成功（即使没有测试，命令也应返回 0）
- `cd frontend && npm run build` 成功

后续 Story 测试约定（提前公布，dev 心中有数）：

- Rust 单元测试：与代码**同文件** `#[cfg(test)] mod tests`
- Rust 集成测试：`crates/<crate-name>/tests/*.rs`
- 前端单元测试：与组件**同目录** `*.test.tsx`，使用 Vitest（Story 1.7 引入）

### Project Structure Notes

**与统一项目结构的对齐：**

- ✅ 6 个子 crate 命名与依赖图严格按 Architecture §结构 定义
- ✅ `crates/` 目录而非顶层散布（避免 IDE 加载混乱）
- ✅ `frontend/` 与 `crates/` 平级（前后端独立构建，未来通过 Story 1.8 的 rust-embed 集成）
- ✅ 根级 Justfile 是唯一开发命令入口（避免散落的 Makefile / shell 脚本）

**潜在变体（已主动规避）：**

- ⚠ 不使用 `members = ["crates/*"]` 通配符 — 显式列出避免误包含临时目录
- ⚠ 不使用 nightly Rust — 锁 stable 1.82，确保团队一致
- ⚠ 不引入 `cargo-make` / `cargo-xtask` — Justfile 已够用，减少工具链
- ⚠ 不在 Story 1.1 引入 antd — 让前端构建产物在 KB 级别（Story 1.7 才引入 ~3MB 的 AntD）

### References

完整的需求与设计来源（Dev 实施时若有疑问可查阅）：

- 项目结构：[Source: `_bmad-output/planning-artifacts/architecture.md` §Project Structure & Boundaries]
- Workspace 决策：[Source: `architecture.md` §Starter Template Evaluation]
- 核心架构决策（版本锁定）：[Source: `architecture.md` §Core Architectural Decisions]
- 命名约定：[Source: `architecture.md` §Implementation Patterns & Consistency Rules / Naming Patterns]
- 跨切关注点：[Source: `architecture.md` §Project Context Analysis / Cross-Cutting Concerns]
- FR 来源：[Source: `_bmad-output/planning-artifacts/prd.md` §Functional Requirements — 注：本 Story 不直接实现任何 FR，是基础设施]
- Epic 上下文：[Source: `_bmad-output/planning-artifacts/epics.md` §Epic 1 / Story 1.1]

### 关键陷阱与避坑提示（必读）

**陷阱 1：Cargo workspace 与子 crate 的 git 关系**
- `cargo new --bin crates/vpn-server` 会自动 `git init` 子目录
- **必须手动删除** `crates/vpn-server/.git`（保留仅根目录 git）
- 否则后续 git status 会忽略子 crate 内容

**陷阱 2：rust-toolchain.toml 与系统 Rust 版本冲突**
- 该文件会强制下载并使用指定 Rust 版本
- dev 机器首次 `cargo build` 可能下载 1.82 工具链（耗时 1-2 分钟）
- 这是预期行为，不要因此切换到 nightly

**陷阱 3：前端 npm vs pnpm vs yarn**
- 本项目统一使用 **npm**（避免锁文件冲突）
- 团队成员若惯用 pnpm/yarn，需自觉用 npm 操作 frontend/
- frontend/package-lock.json 必须提交到 git

**陷阱 4：Vite React-TS 模板的默认内容**
- 默认含 React + Vite logo + 计数器示例
- **必须删除** logo 图片 + 默认 CSS + 计数器代码
- 保持 src/ 最小化（仅 main.tsx + App.tsx）

**陷阱 5：edition 选择**
- 使用 `edition = "2021"`（不用 2024 edition，部分依赖未完全适配）
- 工作区根 Cargo.toml 与每个子 crate 必须一致

### 实施验证清单（Done 前自检）

- [ ] `git status` 显示干净（除新增文件外）
- [ ] `cargo build --workspace` 成功（无 warning 视为通过本 Story；后续 Story 启用 `clippy -D warnings`）
- [ ] `cargo test --workspace` 返回 0（即使没有测试）
- [ ] `cd frontend && npm install && npm run build` 成功
- [ ] `just --list` 显示 4+ 命令
- [ ] 6 个子 crate 目录存在 + 每个有 Cargo.toml + src/
- [ ] `git log` 显示 1 个 commit：`chore: initialize project workspace and frontend skeleton`

## Dev Agent Record

### Agent Model Used

_(由 dev agent 在实施开始时填写：例如 claude-opus-4-7)_

### Debug Log References

_(由 dev agent 填写实施过程中的关键决策与遇到的问题)_

### Completion Notes List

_(由 dev agent 填写完成时的备注，特别是为后续 Story 留下的学习点)_

### File List

_(由 dev agent 填写本次提交涉及的所有文件)_
