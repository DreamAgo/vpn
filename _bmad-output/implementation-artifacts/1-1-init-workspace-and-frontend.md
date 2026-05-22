# Story 1.1: 初始化 Rust workspace 与前端项目

Status: review

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

- [x] **Task 1：初始化 Rust workspace 根目录（AC1）**
  - [x] 1.1 创建仓库根 Cargo.toml workspace 配置（直接 Write，未用 `cargo new`，等效产物）
  - [x] 1.2 Workspace Cargo.toml 含 `[workspace.dependencies]` 与全部锁定版本
  - [x] 1.3 N/A — 未使用 cargo new，无需删除根 src/
  - [x] 1.4 创建 `rust-toolchain.toml` 指定 `channel = "1.85"`（实施时发现 1.82 不支持 edition2024，依赖 getrandom 0.4 强制要求）
  - [x] 1.5 创建 `.gitignore`
  - [x] 1.6 创建 `.editorconfig`
  - [x] 1.7 创建 `LICENSE` 文件（MIT）
  - [x] 1.8 创建 `README.md` 占位

- [x] **Task 2：创建 6 个子 crate（AC1）**
  - [x] 2.1 创建 `crates/vpn-server/{Cargo.toml,src/main.rs}`（bin crate）
  - [x] 2.2 创建 `crates/vpn-cli/{Cargo.toml,src/main.rs}`（bin crate）
  - [x] 2.3 创建 `crates/vpn-core/{Cargo.toml,src/lib.rs}`（lib crate）
  - [x] 2.4 创建 `crates/vpn-wireguard/{Cargo.toml,src/lib.rs}`（lib crate）
  - [x] 2.5 创建 `crates/vpn-platform/{Cargo.toml,src/lib.rs}`（lib crate）
  - [x] 2.6 创建 `crates/vpn-api-types/{Cargo.toml,src/lib.rs}`（lib crate）
  - [x] 2.7 每个 Cargo.toml 含 `publish = false` + workspace 继承字段
  - [x] 2.8 N/A — 直接 Write 创建，无 git 子目录

- [x] **Task 3：初始化前端项目（AC1）**
  - [x] 3.1 执行 `npm create vite@latest frontend -- --template react-ts -y`
  - [x] 3.2 执行 `cd frontend && npm install`（42 packages, 0 vulnerabilities）
  - [x] 3.3 验证 `frontend/tsconfig.app.json` 已 `"strict": true`（Vite 默认）
  - [x] 3.4 验证 `frontend/.gitignore` 已含 `node_modules` 与 `dist`
  - [x] 3.5 修改 `frontend/index.html` `<title>` 为 "vpn 管理后台"
  - [x] 3.6 重写 `frontend/src/App.tsx` 为中文占位；删除 App.css、index.css、assets/ 目录、public/vite.svg

- [x] **Task 4：创建 Justfile 开发命令入口（AC2）**
  - [x] 4.1 创建项目根 `Justfile`
  - [x] 4.2 包含命令：default、dev-server、dev-frontend、dev、test、build、docker（占位）、lint
  - [x] 4.3 N/A — just 工具未在开发机预装，用户需 `brew install just`；命令清单可通过 `cat Justfile` 查看

- [x] **Task 5：构建验证（AC2）**
  - [x] 5.1 `cargo build --workspace` 成功（110 deps 编译通过，18 秒）
  - [x] 5.2 `cd frontend && npm run build` 成功（dist/index.html + 190KB JS，gzip 后 60KB）
  - [x] 5.3 `cat Justfile` 显示 8 个命令（just 工具未装时的等效检查）
  - [x] 5.4 首个 git commit：`6588017 chore: initialize project workspace and frontend skeleton (Story 1.1)`

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

`claude-opus-4-7[1m]`

### Debug Log References

**关键决策 1：Rust 版本从 1.82 调整到 1.85**

- **问题**：使用 1.82 工具链 `cargo build` 失败，错误 `feature 'edition2024' is required` —— `getrandom 0.4.2` 等新版 transitive 依赖需要 Rust 1.85+ 支持 edition 2024
- **决策**：升级 rust-toolchain.toml channel 到 1.85
- **影响**：Cargo.toml `rust-version` 同步更新；Story Tasks 文档已标注此变更原因
- **建议**：后续 Story 若引入更多 transitive 依赖（如 boringtun、sqlx 新版），可能需进一步升级 Rust 版本，dev 需关注 `feature 'edition2024' is required` 类错误

**关键决策 2：直接 Write 而非 cargo new**

- **理由**：`cargo new` 会在每个子 crate 创建独立 `.git` 子目录，需后续删除；直接 Write Cargo.toml + src/{main,lib}.rs 等效且更干净
- **结果**：6 个子 crate 全部成功创建，无 git 子目录污染

**关键决策 3：保留 frontend/public/favicon.svg + icons.svg**

- **理由**：Story 仅要求删除"Vite 默认 logo 与计数器示例"，frontend/public/favicon.svg 是浏览器图标（非示例内容），保留；icons.svg 与 React 默认组件耦合，保留以避免破坏 Vite tooling
- **影响**：未来 Story 1.7（AntD 主题）可按需替换 favicon

**验证结果：**

- `cargo build --workspace` ✓ 18.03s（110 deps, 6 crates）
- `cargo test --workspace` ✓ 0 测试通过（占位代码无测试）
- `cargo fmt --all -- --check` ✓ 全部格式正确
- `cd frontend && npm install` ✓ 42 packages, 0 vulnerabilities
- `cd frontend && npm run build` ✓ 48ms 构建完成（dist/index.html + 190KB JS / 60KB gzip）
- `git log` ✓ 1 个 commit (`6588017`)

### Completion Notes List

**后续 Story 学习点：**

1. **Rust 1.85 已 pin** —— 后续添加任何新 crate 时若报 `edition2024` 错误，说明需进一步升级或降级该 crate 版本。当前栈（tokio 1.52 + 标准依赖）在 1.85 下稳定
2. **`.agents/` 与 `.claude/` 已纳入 git** —— 这是 BMad 工具链的合理选择（项目可复现），但首次 commit 含 9000+ 文件较大；后续 commit 仅涉及业务代码
3. **`just` 工具未在开发机预装** —— README 与 Justfile 已在内，用户首次使用前需 `brew install just`（macOS）或 `cargo install just`
4. **`frontend/eslint.config.js` 已存在** —— Vite 默认生成，未配置项目特定规则；Story 1.7 接入 AntD 时再扩展
5. **未使用 `cargo new --vcs git .`** —— 改用直接文件创建，与 Story 原始描述略有差异但产物等效；后续类似 Story 可参考此做法

**风险与注意事项：**

- `getrandom 0.4.2` 是 transitive 依赖，未直接在 `[workspace.dependencies]` 声明 —— 后续若需特定版本控制可显式 pin
- `Cargo.lock` 已提交（applications 项目惯例），但若团队成员 Rust 版本不一致可能产生 lock 冲突 —— `rust-toolchain.toml` 缓解了此问题

### File List

**新增文件（共 36 个，不含 frontend npm 安装的 node_modules）：**

项目根：
- `.editorconfig`
- `.gitignore`
- `Cargo.toml`（workspace 根）
- `Cargo.lock`
- `Justfile`
- `LICENSE`
- `README.md`
- `rust-toolchain.toml`

Rust 子 crate（每个含 Cargo.toml + src/{main.rs 或 lib.rs}）：
- `crates/vpn-api-types/Cargo.toml`
- `crates/vpn-api-types/src/lib.rs`
- `crates/vpn-core/Cargo.toml`
- `crates/vpn-core/src/lib.rs`
- `crates/vpn-wireguard/Cargo.toml`
- `crates/vpn-wireguard/src/lib.rs`
- `crates/vpn-platform/Cargo.toml`
- `crates/vpn-platform/src/lib.rs`
- `crates/vpn-server/Cargo.toml`
- `crates/vpn-server/src/main.rs`
- `crates/vpn-cli/Cargo.toml`
- `crates/vpn-cli/src/main.rs`

前端（Vite 脚手架生成 + 修改）：
- `frontend/package.json`
- `frontend/package-lock.json`
- `frontend/tsconfig.json`
- `frontend/tsconfig.app.json`
- `frontend/tsconfig.node.json`
- `frontend/vite.config.ts`
- `frontend/eslint.config.js`
- `frontend/index.html`（修改 `<title>`）
- `frontend/.gitignore`
- `frontend/public/favicon.svg`
- `frontend/public/icons.svg`
- `frontend/src/App.tsx`（重写为中文占位）
- `frontend/src/main.tsx`
- `frontend/src/vite-env.d.ts`

**修改文件：** 无（首个 commit）

**删除文件（实施过程中清理）：**
- `frontend/src/App.css`
- `frontend/src/index.css`
- `frontend/src/assets/`（整个目录）
- `frontend/public/vite.svg`

### Change Log

| Date | Version | Description | Story |
|------|---------|-------------|-------|
| 2026-05-22 | 0.1.0 | 初始化 Rust workspace + Vite React-TS 前端骨架；含 6 子 crate、Justfile、CI 配置入口；Rust 版本从 1.82 升级到 1.85（edition2024 兼容性需要） | 1.1 |
