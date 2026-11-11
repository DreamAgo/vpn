---
title: 'VPN 自动模式按持续丢包切换网关'
type: 'feature'
created: '2026-11-11'
status: 'done'
baseline_commit: '778e0410480ee1ac2a58f833e88f64f64f151cc9'
---

<frozen-after-approval reason="用户要求随网络质量自动切换">

## Intent

VPN 连接后也要比较可用网关的网络质量。自动模式优先选择实测丢包最低的网关，并过滤瞬时抖动。

## Boundaries & Constraints

**Always:** 仅比较服务端返回的已授权、就绪网关；后台指定、撤权和隧道故障处理优先。新网关仍需完成 WireGuard 握手。仅默认网关可用时只采样当前网关。

**Release:** 正式版本遵循仓库统一发布流程。

**Never:** 改动现网授权、网关租约或转发规则以迁就客户端选路；根据 VPN 隧道内的探测结果推断外层质量。

| 场景 | 输入 | 预期 |
|---|---|---|
| 持续变差 | 同一备选连续三次评估领先至少 10 个百分点 | 按最低丢包率切换，切换间隔至少一分钟 |
| 瞬时抖动 | 单次丢包或候选丢包率接近 | 保持当前连接 |
| 管理员指定或撤权 | 候选策略变更 | 服务端策略优先，丢弃旧测量 |
| 目标连不上 | 新网关未通过握手 | 按现有故障流程回退 |

</frozen-after-approval>

## Code Map

- `crates/vpn-cli/src/cluster.rs`：CLI 和桌面端共用探测、重连和授权校验。
- `crates/vpn-cli/src/daemon.rs`、`desktop/src-tauri/src/manager.rs`：消费网关重连信号。
- `mobile/android/app/src/main/java/com/biubiu/vpn/GatewayCluster.kt`、`TunnelService.kt`：Android 物理网络探测和重连。

## Tasks & Acceptance

- [x] 共用 Rust 监视器按每 2 秒一个探测、最近 20 次滚动样本计算丢包，每 5 次评估。
- [x] CLI 和桌面端把测得的目标传给重连，在最新候选仍匹配时优先使用。
- [x] Android 采用同一策略，后台采样不阻塞转发。
- [x] 在节点列表展示当前网关及其新鲜指标，绑定设备和网关归属。
- [x] 增加策略回归测试，记录使用方式和验证范围。

给定至少两个获授权的就绪网关，当当前网关持续丢包且备选达到切换门槛时，客户端应自动尝试切换；给定设备已指定网关，当备选质量更好时，客户端应保留指定策略。

## Spec Change Log

- 用户补充要求在「节点」列表显示设备到当前网关的实测延迟、丢包率。指定网关或仅有默认网关的设备也采样当前网关。以成功 UDP 应答的平均往返延迟及最近 20 次的丢包率上报，切换后不显示上一网关的测量。

## Verification

CLI 测试、桌面端 Rust 检查、Android 单元测试、服务端库测试与前端构建通过。覆盖短时丢包、切换阈值、目标失联、授权变化、网关握手确认及报告过期。真机双网关在线切换尚待联调。

## Suggested Review Order

- 客户端保持连接时持续采样，切换前复核策略。
  [cluster.rs:453](../../crates/vpn-cli/src/cluster.rs#L453)

- 服务端按已确认网关和租约展示近期测量。
  [cluster.rs:814](../../crates/vpn-server/src/cluster.rs#L814)

- 节点列表在虚拟 IP 后显示当前网关及质量。
  [PeersPage.tsx:153](../../frontend/src/pages/PeersPage.tsx#L153)

- Android 按物理网络测量并比较滚动样本。
  [GatewayQuality.kt:46](../../mobile/android/app/src/main/java/com/biubiu/vpn/GatewayQuality.kt#L46)

- 验证只在持续丢包改善后切换。
  [quality.rs:305](../../crates/vpn-cli/src/cluster/quality.rs#L305)
