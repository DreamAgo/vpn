# 真机收尾清单

本项目在无 root / 无真实网卡 / 单一开发平台（macOS）的环境下完成了**控制平面 + 可测逻辑**的全部实现与测试。以下条目属于**系统集成层**，必须在目标平台真机（或对应 OS 的 CI runner）上完成与验证。它们当前以可编译的骨架、`NoopWireGuardControl` 注入或 `#[ignore]` 测试形式存在，不影响其余功能与全部自动化测试通过。

## 0. Docker 部署 + Docker 网络组网（✅ 真机验证 2026-05-25 @ 192.168.188.89）

容器化部署（bridge + 发布端口，零接触宿主现有业务）：
- 运行时镜像须与 builder 同 glibc：`rust:1.90-slim` 现为 trixie（glibc 2.39），运行时用 `debian:trixie-slim`（bookworm 的 2.36 会 `GLIBC_2.39 not found`）。
- 运行时镜像装 `wireguard-tools iproute2 iptables`；容器 `--cap-add NET_ADMIN --sysctl net.ipv4.ip_forward=1`，发布 `8889/tcp` + `51820/udp`。
- entrypoint 先 `iptables -t nat -A POSTROUTING -s <VPN_SUBNET> ! -d <VPN_SUBNET> -j MASQUERADE` 再 exec vpn-server（让 VPN 客户端流量以网关身份进 docker 网络，回程自动）。
- 容器 **join 目标 docker 网络** + `VPN_SERVER_ROUTES=<docker子网>`：服务端经 `allowed_routes` 把该网段下发给客户端，客户端零手工配网段即可访问。
- 内核 WireGuard 接口在**容器 netns**内创建可用；UDP 经发布端口 DNAT，WireGuard 握手不受影响；容器 netns 内 FORWARD 默认 ACCEPT（无需处理宿主 FORWARD DROP）。

验证：VPN 客户端经隧道 ping 隔离 docker 网络（172.31.100.0/24）里的 nginx 容器 0% 丢包 + HTTP 200。

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
- [x] **异地组网（站点互通）已真机验证（2026-05-25）**：两个不同用户的客户端经服务端 wg0 互 ping，4/4 0% 丢包，`ttl=63`（经服务端转发 -1）。需要的服务端配置：
  - `sysctl -w net.ipv4.ip_forward=1`
  - `iptables -I FORWARD -i wg0 -o wg0 -j ACCEPT` —— **关键**：装了 Docker 的主机 `FORWARD` 链默认策略是 `DROP`，不加这条规则 peer 间转发会被全部丢弃（排查耗时点）。
  - 若客户端需访问公网/内网（全隧道），还需对出口网卡做 MASQUERADE：`iptables -t nat -A POSTROUTING -s 10.8.0.0/24 -o <eth> -j MASQUERADE`。
  - 生产建议：把这些规则放进部署脚本 / systemd `ExecStartPost`（见 `packaging/linux`），或由运维固化；应用本身不擅自改主机防火墙。
- [x] **站点 LAN 网段路由（可配置路由，非全转发）已真机验证（2026-05-25）**：peer 注册时 `routed_subnets` 声明背后 LAN（如 `192.168.20.0/24`），服务端 `KernelWireGuardControl` 自动把网段加入该 peer 的 allowed-ips **并加 `ip route <subnet> dev wg0`**（`wg set` 不像 `wg-quick` 自动加路由，这是关键）。客户端从注册响应的 `allowed_routes` 获知应路由的网段（VPN 子网 + 各站点 LAN），实现分隧道。验证：clientA 经服务端中转 + 站点网关 B 转发，ping 通 B 内网主机 192.168.20.1，0% 丢包。
  - 站点网关侧仍需自行开 `ip_forward` 并把 VPN 流量转发/MASQUERADE 进其本地 LAN。
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
