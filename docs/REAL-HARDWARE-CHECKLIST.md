# 真机收尾清单

本项目在无 root / 无真实网卡 / 单一开发平台（macOS）的环境下完成了**控制平面 + 可测逻辑**的全部实现与测试。以下条目属于**系统集成层**，必须在目标平台真机（或对应 OS 的 CI runner）上完成与验证。它们当前以可编译的骨架、`NoopWireGuardControl` 注入或 `#[ignore]` 测试形式存在，不影响其余功能与全部自动化测试通过。

## 1. WireGuard 真实数据平面

### ✅ 服务端：已用内核 WireGuard 实现并真机验证（2026-05-25 @ 192.168.188.89）

不走 boringtun，改用 **Linux 内核 WireGuard**（`vpn_wireguard::KernelWireGuardControl`，经 `ip`/`wg` 命令操作内核接口），**彻底规避了 boringtun↔x25519-dalek 依赖冲突**。通过 `VPN_WG_BACKEND=kernel` 启用（默认 `noop`）。

真机验证结果：
- 启动自动创建 `wg0`（10.8.0.1/24，监听 UDP 51820，服务端公钥就绪）；
- `POST /peers/register` → 内核 `wg0` 实时新增 peer（`wg show` 可见 allowed-ips）；
- 网络命名空间客户端建真实隧道，**握手成功 + ping 10.8.0.1 0% 丢包（~0.3ms）**；
- 启动恢复：`build_peer_service_with_backend` 会把 active peers 重新下发到内核接口。

运行要求：Linux + 内核 WireGuard + `wireguard-tools` + root/CAP_NET_ADMIN。

### 待办（客户端 + 进阶）
- [ ] 客户端 `vpn-cli` daemon 数据面：当前 `run_data_plane` 为骨架。两条路线任选——
  (a) 客户端也用内核/`wg-quick`（Linux/macOS 有 wg 工具时最简单，可复用 `GET /peers/me/config` 下载的 .conf）；
  (b) 用户态 boringtun（需先解决 `boringtun 0.6` 要求 `x25519-dalek =2.0.0-rc.3` 的冲突：workspace 锁版本，或客户端独立 crate 隔离依赖）。
- [ ] **异地组网（站点互通）**：两个客户端经服务端互 ping，需 `net.ipv4.ip_forward=1` + 服务端在 peers 间转发（allowed-ips 已覆盖整个子网，开转发即可，待验证）。
- [ ] Story 4.4 boringtun timer task：仅当选用户态后端时需要；内核后端不涉及。
- [ ] 容器化部署内核后端：容器需 `--cap-add NET_ADMIN` + host 内核 WireGuard；UDP 端口建议 `network_mode: host` 避免 NAT 破坏握手。

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
