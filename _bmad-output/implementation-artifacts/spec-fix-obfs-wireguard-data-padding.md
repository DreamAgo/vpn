---
title: '修复混淆隧道 WireGuard 数据包填充'
type: 'bugfix'
created: '2026-08-25'
status: 'done'
baseline_commit: '3ff0f45e29df702bfe7b70424f09e679d9066df0'
context:
  - '{project-root}/_bmad-output/implementation-artifacts/spec-udp-encrypted-obfuscation.md'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** Linux test 客户端能够完成混淆层和 WireGuard 握手，但业务数据无法到达服务端。`boringtun 0.7.1` 直接加密原始 TUN 报文而不执行 WireGuard 规定的 16 字节填充，导致典型 84 字节 ICMP 报文形成 116 字节 WireGuard Data 包，并被混淆层的严格格式校验拒绝；32 字节 keepalive 可通过，因此连接表面正常。

**Approach:** 在客户端把送入 `Tunn::encapsulate` 的 IP 明文以零填充到 16 字节边界，使 boringtun 生成规范长度的 WireGuard Data 包；流量统计仍使用原始 IP 报文长度。保留混淆层严格校验，并补充可定位单包编码/发送失败的安全日志。

## Boundaries & Constraints

**Always:** 填充仅应用于非空 TUN 业务报文；使用零字节；验证缓冲区容量而不是发生 panic；原始 IP 头和长度字段保持不变；直连与两种混淆模式共用同一规范化路径；发送失败必须记录阶段、WireGuard 类型、长度和脱敏错误；保持单 UDP 端口和现有线格式不变。

**Ask First:** 升级或 fork boringtun；放宽 `vpn-obfs` 的 WireGuard 格式校验；修改 MTU、WireGuard 密钥、服务端内核接口或混淆协议格式。

**Never:** 记录载荷或密钥；把填充计入用户流量；吞掉握手后队列排空的发送错误；通过禁用格式验证掩盖非规范报文；改动 `outputs/`。

## I/O & Edge-Case Matrix

| 场景 | 输入 / 状态 | 预期行为 | 错误处理 |
|---|---|---|---|
| 非对齐业务包 | 84 字节 IPv4 ICMP | 补至 96 字节后封装为 128 字节 WG Data | 编码并发送成功，统计 84 字节 |
| 已对齐业务包 | 16、96 或 1424 字节 | 内容和长度不变 | 正常发送 |
| 空报文 | 握手或 keepalive 触发 | 不增加人为明文填充 | 保持 boringtun 行为 |
| 容量不足 | 对齐后超过缓冲区 | 不调用 boringtun | 返回明确数据面错误并安全停隧道 |
| 发送失败 | 编码或 UDP send 失败 | 不谎报 TX 成功 | 限频记录具体阶段和长度，继续按现有恢复策略运行 |

</frozen-after-approval>

## Code Map

- `crates/vpn-cli/src/wg_userspace.rs` -- TUN 读取、boringtun 封装、握手后队列排空、混淆编码与 UDP 发送。
- `crates/vpn-obfs/src/lib.rs` -- WireGuard Data 长度严格校验及两种混淆模式编码。
- `boringtun 0.7.1 noise/session.rs` -- 上游 `format_packet_data` 未实现 16 字节 padding 的已知行为。

## Tasks & Acceptance

**Execution:**
- [x] `crates/vpn-cli/src/wg_userspace.rs` -- 增加无 panic 的 16 字节零填充 helper，在所有 TUN 业务报文进入 boringtun 前调用，并按原始长度统计 TX。
- [x] `crates/vpn-cli/src/wg_userspace.rs` -- 让常规发送和握手后队列排空保留具体错误，按阶段限频输出安全诊断，避免再次把编码失败误判为网络阻断。
- [x] `crates/vpn-cli/src/wg_userspace.rs` / 测试 -- 覆盖空、边界、已对齐、84 字节和容量不足；用两端 `Tunn` 验证 84 字节 IPv4 报文产生 128 字节 Data，混淆编解码成功且解密后的 IP 长度语义保持 84。

**Acceptance Criteria:**
- Given test 客户端已登录并获得 `10.9.0.2`，when Ping `10.9.0.1`，then 收到响应、服务端 `wgtest0` 能捕获 ICMP 且客户端 TX/RX 增长。
- Given 混淆启用或关闭，when 发送任意合法 MTU 内 IPv4/IPv6 TUN 报文，then WireGuard Data 长度均满足 16 字节边界且不存在回归。
- Given 编码或 UDP 发送失败，when 查看日志，then 能区分 padding、混淆编码和 socket 发送阶段，且不包含敏感载荷。

## Spec Change Log

## Design Notes

填充发生在 WireGuard 加密之前，因为规范要求的是加密明文的零填充。对端 WireGuard 解密后会依据 IPv4 `total_length` 或 IPv6 `payload_length` 识别真实 IP 包长度，因此额外零字节不会进入操作系统协议栈。不能在混淆层对密文尾部补零，否则会破坏 Poly1305 标签；也不能放宽校验，否则会永久接受非规范 WireGuard 数据报。

## Verification

**Commands:**
- `cargo fmt --all -- --check` -- 格式检查通过。
- `cargo test -p vpn-cli -p vpn-obfs` -- padding、boringtun 往返及混淆回归通过。
- `cargo clippy -p vpn-cli -p vpn-obfs --all-targets -- -D warnings` -- 无警告。
- 2026-08-25 真机执行 `ping -c 3 10.9.0.1` -- 3/3 响应、0% 丢包、平均 21.1 ms。
- 服务端 `wgtest0` 抓包 -- 3 个请求和 3 个回复，客户端状态累计 `↓252 B / ↑556 B`，无 padding/编码/UDP 发送错误。

## Suggested Review Order

**规范化与发送入口**

- 先解析完整 WireGuard 类型，再驱动安全日志和统计。
  [`wg_userspace.rs:107`](../../crates/vpn-cli/src/wg_userspace.rs#L107)

- 加密前零填充并显式检查容量，保持原 IP 长度语义。
  [`wg_userspace.rs:116`](../../crates/vpn-cli/src/wg_userspace.rs#L116)

- TUN 入口统一填充、封装和原始长度统计。
  [`wg_userspace.rs:549`](../../crates/vpn-cli/src/wg_userspace.rs#L549)

**会话恢复与错误诊断**

- 定时器错误同步清理待统计队列，避免跨会话污染。
  [`wg_userspace.rs:685`](../../crates/vpn-cli/src/wg_userspace.rs#L685)

- 握手响应后排空队列，并保留发送失败与原始长度。
  [`wg_userspace.rs:840`](../../crates/vpn-cli/src/wg_userspace.rs#L840)

- 混淆编码与 UDP 发送使用不同安全错误阶段。
  [`wg_userspace.rs:910`](../../crates/vpn-cli/src/wg_userspace.rs#L910)

**回归测试**

- 完整两端握手验证 84 字节包在两种模式下往返。
  [`wg_userspace.rs:1099`](../../crates/vpn-cli/src/wg_userspace.rs#L1099)
