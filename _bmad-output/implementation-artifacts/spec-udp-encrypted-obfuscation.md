---
title: 'Rust 原生 WireGuard UDP 混淆传输'
type: 'feature'
created: '2026-08-24'
status: 'done'
baseline_commit: 'a9fdeab921866e51909fb9343b919dd3e83b5c97'
context:
  - '{project-root}/_bmad-output/planning-artifacts/architecture.md'
  - '{project-root}/_bmad-output/planning-artifacts/prd.md'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** 状态型 DPI 会识别并丢弃标准 WireGuard 握手；换端口不稳定，外置代理增加部署和跨平台成本。

**Approach:** 以 swgp-go v1.10.0 两种 2026 模式和 `IP:port` 会话模型为基准，在 Rust 内独立实现不兼容线格式；仅加固资源耗尽与密钥隔离。

## Boundaries & Constraints

**Always:** 提供低开销 v1（默认）与全填充 v1；无 `session_id`、序号或方向字段；按外层 `IP:port` 建独立 WG socket，180 秒回收；握手使用随机 XChaCha nonce、±15 秒时间戳、30 秒重放池；数据重放与认证交给 WireGuard；公网单 UDP 端口；三平台共用 crate；密钥来自环境/HTTPS 响应并 `Zeroizing`；失败不回退原生 WG。

**Ask First:** 改为每节点密钥、修改 v1 线格式、增加公网端口、数据库/UI 或防火墙变更。

**Never:** 复制/链接 swgp-go；自制密码原语；把混淆层当节点认证；记录密钥或载荷；无效包回复；支持缺少 2026 重放保护的 legacy 模式。

## I/O & Edge-Case Matrix

| 场景 | 输入/状态 | 预期行为 | 错误处理 |
|---|---|---|---|
| 未启用 | 无 transport | 现有 WG 直连不变 | 无回归 |
| 低开销 | 有效 v1 | 数据零增量，握手加密填充 | 分阶段日志 |
| 全填充 | paranoid v1 | 每包 AEAD 且固定为路径上限 | 自动降低 MTU |
| NAT 变化 | 新 `IP:port` | 新建代理会话，WG 自行漫游 | 旧会话超时回收 |
| 恶意数据 | 随机/篡改/重放/洪泛 | 不进入 WG、不建无限会话 | 静默丢弃、聚合计数 |
| 配置错误 | HTTP 密钥或旧客户端 | 拒绝连接/注册 | 明确安全错误 |

</frozen-after-approval>

## Code Map

- `crates/vpn-obfs/`、根 `Cargo.toml` -- 两种 v1 handler、HKDF、重放池、WG 报文校验。
- `crates/vpn-api-types/src/peer.rs`、`crates/vpn-server/src/{config,services/peer_service}.rs` -- capability、模式、endpoint、PSK 契约。
- `crates/vpn-cli/src/{daemon,wg_userspace}.rs` -- boringtun UDP 前后编解码及动态 MTU。
- `crates/vpn-server/src/{main,udp_obfs}.rs` -- 公网监听、NAT 表、内部 socket、限速和清理。
- `docker/`、`docs/` -- 端口、升级、时钟与诊断。

## Tasks & Acceptance

**Execution:**
- [x] `crates/vpn-obfs/` -- 独立实现两模式；HKDF 按方向及 AES/AEAD 用途派生密钥；拒绝 `<16B`、未知 WG type、非零 reserved、非法固定长度及超 MTU 包；覆盖向量、篡改、重放和边界测试。
- [x] API/配置 -- 增加 `obfs-v1` capability 和可选 transport；启用时拒绝旧客户端、非 HTTPS 自动下发和非法 PSK/MTU。
- [x] `crates/vpn-server/src/udp_obfs.rs` -- 解码通过后才按源 `IP:port` 建 socket；默认最多 4096 会话、每 IP 每分钟新建 20 个，180 秒回收。
- [x] 客户端数据面 -- 路径允许时低开销模式保持 1420 MTU；全填充按 IP/UDP、外层 42B 和 WG 32B 开销计算并按 16 向下对齐；切网重建本地会话。
- [x] Docker/文档/日志 -- 默认仅发布 `47358/udp`，51820 不发布；记录模式、丢弃计数、会话生命周期和时钟错误。

**Acceptance Criteria:**
- Given 三平台同一模式，when 连接，then 均与 v1 服务端互通且状态一致。
- Given 受限外网，when 新建低开销会话，then `10.8.0.1` 双向可达且公网仅一个 UDP 端口。
- Given NAT 换址，when 新源发送有效 WG 包，then 新会话恢复通信，旧会话按时回收且不串流。
- Given 随机、伪造类型、篡改、过期、重放或洪泛，when 服务端接收，then 不越过校验/限速边界。
- Given 两种模式互测，when 传输最大包，then 无 IP 分片、截断或 MTU 黑洞。

## Spec Change Log

- 2026-08-24：改为完整参考 swgp-go 2026 两模式和无 session_id 会话模型；保留独立格式，并增加报文校验、资源上限及 HKDF 密钥隔离。

## Design Notes

低开销数据为 `AES(first16)||WG[16..]`；握手为 `AES(first16)||AEAD(WG[16..]||padding||timestamp:u64be||len:u16be, AAD=first16)||nonce[24]`。全填充为 `nonce[24]||AEAD(len:u16be||timestamp?:u64be||WG||padding)`，总长等于按路径 MTU 算出的 UDP payload。与 swgp-go 的原始 PSK、元数据顺序/字节序不同，不能互通。解码后的 WG type 必须是 little-endian `1..4`（reserved 为零），并校验标准握手长度或数据长度，避免随机 UDP 创建代理会话。

## Verification

**Commands:**
- `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings` -- 质量门通过。
- `cargo test --workspace` -- 两模式、API、重放、NAT、限速和直连回归通过。
- Ruby YAML 解析及端口断言 -- Compose 仅公开混淆 UDP；本机缺少 Compose 插件，未运行官方 `docker compose config`。
- 真机测试 -- 待在受限 Linux、Windows/macOS 和真实 WireGuard/TUN 环境验证两模式、切网、重连、无分片与清理。

## Suggested Review Order

**协议与密码边界**

- 从共享 Codec 理解两种独立线格式、方向密钥与严格 WG 校验。
  [`lib.rs:115`](../../crates/vpn-obfs/src/lib.rs#L115)

- 低开销模式保持数据零增量，并为握手加入 AEAD 元数据。
  [`lib.rs:196`](../../crates/vpn-obfs/src/lib.rs#L196)

- 全填充模式固定外层长度并将所有包纳入 AEAD。
  [`lib.rs:298`](../../crates/vpn-obfs/src/lib.rs#L298)

**服务端会话与资源控制**

- 公网入口按源 `IP:port` 隔离 socket、限速、回收并聚合丢弃。
  [`udp_obfs.rs:114`](../../crates/vpn-server/src/udp_obfs.rs#L114)

- 绑定阶段隔离方向密钥，并按实际 IP 族计算外层上限。
  [`udp_obfs.rs:72`](../../crates/vpn-server/src/udp_obfs.rs#L72)

**客户端数据面**

- 客户端验证下发 PSK/模式/MTU，再构造双向 codec。
  [`wg_userspace.rs:61`](../../crates/vpn-cli/src/wg_userspace.rs#L61)

- 动态 MTU 覆盖低路径与全填充模式，防止分片和黑洞。
  [`wg_userspace.rs:88`](../../crates/vpn-cli/src/wg_userspace.rs#L88)

- 转发循环统一封装收发、动态缓冲并保持失败不降级。
  [`wg_userspace.rs:397`](../../crates/vpn-cli/src/wg_userspace.rs#L397)

**控制面、安全配置与部署**

- 启用时强制 HTTPS、合法 PSK/IPv4 endpoint、MTU 与真实 WG 后端。
  [`config.rs:207`](../../crates/vpn-server/src/config.rs#L207)

- capability 门禁及 Zeroizing DTO 将 transport 安全下发给新客户端。
  [`peer_service.rs:238`](../../crates/vpn-server/src/services/peer_service.rs#L238)

- Compose 默认真实 WG 后端，公网只映射单个混淆 UDP 端口。
  [`docker-compose.yml:27`](../../docker/docker-compose.yml#L27)

- API 类型固定模式名称和可清零 PSK 契约。
  [`peer.rs:41`](../../crates/vpn-api-types/src/peer.rs#L41)
