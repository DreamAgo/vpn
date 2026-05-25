# 真机收尾清单

本项目在无 root / 无真实网卡 / 单一开发平台（macOS）的环境下完成了**控制平面 + 可测逻辑**的全部实现与测试。以下条目属于**系统集成层**，必须在目标平台真机（或对应 OS 的 CI runner）上完成与验证。它们当前以可编译的骨架、`NoopWireGuardControl` 注入或 `#[ignore]` 测试形式存在，不影响其余功能与全部自动化测试通过。

## 1. WireGuard 真实数据平面（最高优先）

**现状**：`vpn_wireguard::WireGuardControl` 已抽象增删 peer；服务端注入 `NoopWireGuardControl`（只在内存记账，不碰网卡），所有 peer API 据此通过集成测试。`render_client_config` 可生成标准 `.conf`，「接入指南」页支持下载，可临时用官方 WireGuard 客户端导入接入。

**待办**：
- [ ] **解决依赖冲突**：`boringtun 0.6` 要求 `x25519-dalek =2.0.0-rc.3`，与当前 `vpn-wireguard` 的 `x25519-dalek ^2`(2.0.1) 不兼容。二选一：
  - workspace 统一锁 `x25519-dalek = "=2.0.0-rc.3"`；或
  - 改用 `defguard_wireguard_rs`（内核态，Linux 性能更好，PRD/架构原始选型）。
- [ ] 实现 `BoringTunControl`（或内核态等价物）落实 `WireGuardControl`：创建 `wg0` 接口、配置服务端公私钥、监听 UDP，并把 peer 增删落到真实接口。
- [ ] Story 4.4：boringtun timer task（每 100ms `update_timers`，处理 `TunnResult`，task 崩溃自动重启）——规避静默断线。
- [ ] 客户端 `vpn-cli` daemon 数据面转发（`run_data_plane` 骨架）：TUN ↔ WireGuard ↔ UDP 真实收发。
- [ ] 服务端启动恢复在真实接口上 reload 所有 active peer（逻辑已在 `peer_service`，需接真实 control）。

## 2. 客户端三平台系统集成（vpn-platform，均有骨架 + `#[ignore]` 测试）

- [ ] **TunDevice**：在 Linux(`CAP_NET_ADMIN`)/macOS(utun)/Windows(WinTun 驱动) 真机创建设备并 `configure_ip`，跑通 `#[ignore]` 的 `open_real_device`。
- [ ] **CredentialStore**：在有桌面会话/已解锁钥匙串的真机验证 `KeyringCredentialStore` 读写（`#[ignore]` 的 `keyring_round_trip`）。文件降级路径已可单测。
- [ ] **DaemonRuntime**：真机执行 `systemctl --user` / `launchctl` / `sc.exe` 注册并验证服务运行（单元/plist 渲染已单测）。

## 3. 打包与安装（packaging/，脚手架已就绪）

- [ ] Linux：在 Linux 上用 `nfpm` 构建 `.deb`/`.rpm`，验证 systemd system 服务安装/启动 + `CAP_NET_ADMIN`。
- [ ] macOS：用 `pkgbuild/productbuild` 构建 `.pkg`，完成**代码签名 + 公证**（需 Apple 开发者证书）。
- [ ] Windows：用 Inno Setup（ISCC）构建安装器，验证 PATH + 服务注册（需 Windows + WinTun）。

## 4. 端到端测试（Story 6.6）

- [ ] 搭建双节点真机/容器拓扑（一台服务端 + 两台客户端），验证：登录 → 连接 → 节点互通 ping → 断网重连 → 强制下线生效。
- [ ] 重连成功率 ≥ 99%（PRD NFR）。

## 5. 发布前安全与覆盖率（Story 6.8 / 6.9）

- [ ] `cargo deny check`（配置见 `deny.toml`）+ `cargo audit` 无高危。
- [ ] 测试覆盖率达标（建议接入 `cargo llvm-cov`，目标见 PRD）。
- [ ] 第三方/内部安全审计 + 小范围真实用户测试。

---

> 维护提示：完成某项后，把对应 Story 在 `_bmad-output/implementation-artifacts/sprint-status.yaml` 标记为 `done`，并在此清单勾选。
