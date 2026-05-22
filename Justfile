# vpn 项目开发命令入口
# 安装 just：https://github.com/casey/just（macOS：brew install just）

# 默认显示命令列表
default:
    @just --list

# 开发：仅启动后端（含 cargo watch 自动重载，需安装 cargo-watch）
dev-server:
    cargo watch -x 'run --bin vpn-server'

# 开发：仅启动前端（Vite dev server）
dev-frontend:
    cd frontend && npm run dev

# 开发：同时启动前端 + 后端
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
