# syntax=docker/dockerfile:1.7
#
# vpn 服务端 Docker 镜像（多阶段构建）
#
# 阶段 1：planner — cargo-chef 生成依赖 recipe
# 阶段 2：cacher — 基于 recipe 缓存依赖编译
# 阶段 3：frontend-builder — 构建前端 dist/
# 阶段 4：builder — 编译 Rust 二进制（含嵌入前端）
# 阶段 5：runtime — 最终镜像（debian-slim + 非 root + CAP_NET_ADMIN）

ARG RUST_VERSION=1.90

# ===== Stage 1: planner =====
FROM lukemathwalker/cargo-chef:latest-rust-${RUST_VERSION} AS planner
WORKDIR /app
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# ===== Stage 2: cacher =====
FROM lukemathwalker/cargo-chef:latest-rust-${RUST_VERSION} AS cacher
WORKDIR /app
# cargo chef cook 会编译整个 workspace 依赖，含 vpn-cli 的 libdbus-sys(keyring) → 需 libdbus。
RUN apt-get update \
    && apt-get install -y --no-install-recommends libdbus-1-dev pkg-config \
    && rm -rf /var/lib/apt/lists/*
COPY --from=planner /app/recipe.json recipe.json
# 缓存所有依赖（仅在 recipe.json 变化时重建）
RUN cargo chef cook --release --recipe-path recipe.json

# ===== Stage 3: frontend-builder =====
FROM node:22-slim AS frontend-builder
WORKDIR /app/frontend
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci
COPY frontend/ ./
RUN npm run build

# ===== Stage 4: builder =====
FROM lukemathwalker/cargo-chef:latest-rust-${RUST_VERSION} AS builder
WORKDIR /app
# 与 cacher 一致的系统库（复用其编译产物时链接阶段可能需要）。
RUN apt-get update \
    && apt-get install -y --no-install-recommends libdbus-1-dev pkg-config \
    && rm -rf /var/lib/apt/lists/*
# 复用 cacher 的 target 目录（依赖已编译）
COPY --from=cacher /app/target target
COPY --from=cacher /usr/local/cargo /usr/local/cargo
# 复用 frontend dist 用于 rust-embed
COPY --from=frontend-builder /app/frontend/dist frontend/dist
COPY . .
# SQLX_OFFLINE 避免 build 时需要 DATABASE_URL（CI 友好）
ENV SQLX_OFFLINE=true
RUN cargo build --release --bin vpn-server

# ===== Stage 5: runtime =====
FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        libssl3 \
    && rm -rf /var/lib/apt/lists/*

# 创建非 root 用户
RUN groupadd --system --gid 1000 vpn \
    && useradd --system --uid 1000 --gid vpn --shell /usr/sbin/nologin vpn

# 数据目录（持久化卷挂载点）
RUN mkdir -p /var/lib/vpn && chown vpn:vpn /var/lib/vpn

WORKDIR /app
COPY --from=builder /app/target/release/vpn-server /app/vpn-server
# migrations 与二进制一起携带（启动时由 sqlx migrate 加载）
COPY --from=builder /app/migrations /app/migrations

USER vpn

EXPOSE 80 443 8080 51820/udp

ENV VPN_BIND_ADDR=0.0.0.0:8080
ENV VPN_DATA_DIR=/var/lib/vpn
ENV DATABASE_URL=sqlite:///var/lib/vpn/vpn.db?mode=rwc
ENV RUST_LOG=info

ENTRYPOINT ["/app/vpn-server"]
