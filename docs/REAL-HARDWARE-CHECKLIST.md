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
- [x] **客户端 `vpn-cli` daemon 数据面（Linux 内核后端）已真机验证（2026-05-25 @ 192.168.188.89）**：
  采用路线 (a)——客户端走内核 WireGuard（`vpn_wireguard::KernelClientTunnel`，经 `ip`/`wg` 操作内核接口）。
  daemon `run()` 的 `Connect` 分支现实际驱动：`connect_once`（注册取 vpn_ip + `allowed_routes`）→ `bring_up_tunnel`
  （创建 `vpncli0` + 配 peer/endpoint/allowed-ips + 加 `ip route`）→ `tokio::spawn(run_heartbeat)`（每 30s 自动心跳）；
  `Disconnect` 分支停心跳 + `KernelClientTunnel::down` 拆接口。验证：容器内 `vpn-cli daemon run` + `vpn-cli up`
  → wg 握手成功、ping 10.8.0.1 0% 丢包、**后台节点自动转 online 且 `last_seen` 每 30s 推进一次**；`vpn-cli down`
  后 `last_seen` 立即冻结、接口删除。运行要求：Linux + root/`CAP_NET_ADMIN` + `wireguard-tools` + `iproute2`。
  容器内还需 `libdbus-1-3`（keyring 动态链接）；headless 用 `VPN_CLI_CRED_BACKEND=file` 走加密文件凭证后端。
- [x] **路线 (b) 用户态 boringtun 已实现并真机验证（✅ macOS + ✅ Linux 2026-05-26），全平台统一、零外部依赖**：
  依赖冲突已消解——`boringtun 0.7` 用 `x25519-dalek 2.0.1`，与 `vpn-wireguard` 同版本，不再有 `0.6` 的 rc 锁。
  新模块 `vpn_cli::wg_userspace`：`boringtun::noise::Tunn`（握手/加解密/keepalive）+ `tun` crate（跨平台 TUN：
  Linux `/dev/net/tun`、macOS `utun`、Windows WinTun dll）+ `net-route`（跨平台路由）+ tokio UDP，单任务
  `select!` 转发（TUN→encapsulate→UDP / UDP→decapsulate→TUN / 定时 update_timers）。daemon `bring_up_tunnel`
  现**全平台统一**调它，**不再 shell-out 到 `wg`/`wg-quick`/`wireguard.exe`**——客户端单二进制即可建隧道，
  仅需 root/管理员开 TUN（Windows 另需随包 `wintun.dll`）。`Disconnect` 经 watch 信号停转发任务并删路由。
  **真机验证（macOS，本机无 wg 工具）**：纯 `vpn-cli` 二进制 `daemon run`+`up` → utun 拿到 10.8.0.2、
  boringtun 握手成功、`ping 10.8.0.1` 4/4 0% 丢包、`down` 后路由自动撤除。证明零外部依赖成立。
  - 经验：macOS utun 是点对点接口，**不会**像 Linux 那样由接口地址自动产生子网连通路由——必须把 VPN 子网
    也显式经 `net-route`(ifindex) 加入路由表，否则发往服务端 VPN IP 的包走默认网卡（表现为握手起来了但 ping 不通）。
  - macOS utun 的 4 字节协议头由 `tun` crate 自动 strip/prepend（`packet_information` 默认 true），boringtun 见到的是裸 IP 包。
  - **Linux 真机验证（✅ 2026-05-26 @ 192.168.188.54，Ubuntu 24.04）**：在**无任何 wg 工具、无 `ip`/`iptables`** 的
    minimal debian 容器（`--cap-add NET_ADMIN --device /dev/net/tun`）里跑纯 `vpn-cli` 二进制 → `ping 10.8.0.1` 4/4
    0% 丢包（ttl=64）、`ping 172.31.100.2`（docker 网段 nginx，经服务端网关）3/3 0% 丢包（ttl=63，分流路由也通）。
    Linux 上 `10.8.0.0/24` 显式加路由会报 `File exists`（接口地址已自动产生连通路由），**无害**，已忽略。
  - 188.89 是 Proxmox LXC，无 `/dev/net/tun` 且无法 modprobe，不能在其上验证用户态（故改用 188.54）。Windows 待验证。
  - **公网异地组网端到端（✅ 2026-05-26）**：服务端映射到公网 `223.84.157.49`，Linux 网关(188.54)声明 `--route 192.168.186.0/24`，
    mac 从**真外网(蜂窝)**经公网登录+建隧道 → ping 内网 `192.168.186.1`：连前 100% 丢包、连后 4/4 0% 丢包 ttl=61(三跳转发)。
    链路 mac(外网)→公网→服务端→Linux网关→内网，全程零依赖用户态。
  - 新增 `VPN_ENDPOINT_OVERRIDE` env（split-horizon）：内网节点用内网 endpoint 避开公网 UDP NAT 回环；外网节点用服务端下发的公网 endpoint。
  - 部署铁律：① 不能路由"服务端自己所在的 LAN"（服务端会把自身回包导进 wg0，断本网段）；② 同 LAN 连自己公网 IP 的 NAT 回环(UDP)常不通，真外网才作数；③ rsync 保旧 mtime 致 cargo 不重编，远程构建前 `touch` 源文件。
  - 早期路线 (a)（macOS `wg-quick` / Windows `wireguard.exe` / Linux 内核 `wg` shell-out）已被 (b) 取代并从客户端移除；
    其遗留的客户端类型 `vpn_wireguard::KernelClientTunnel`（上文 2026-05-25 条目所述，daemon 已不再调用）
    已于 2026-06-29 作为死代码删除——客户端数据面只剩用户态 boringtun 一条路径。
- [x] **异地组网（站点互通）已真机验证（2026-05-25）**：两个不同用户的客户端经服务端 wg0 互 ping，4/4 0% 丢包，`ttl=63`（经服务端转发 -1）。需要的服务端配置：
  - `sysctl -w net.ipv4.ip_forward=1`
  - `iptables -I FORWARD -i wg0 -o wg0 -j ACCEPT` —— **关键**：装了 Docker 的主机 `FORWARD` 链默认策略是 `DROP`，不加这条规则 peer 间转发会被全部丢弃（排查耗时点）。
  - 若客户端需访问公网/内网（全隧道），还需对出口网卡做 MASQUERADE：`iptables -t nat -A POSTROUTING -s 10.8.0.0/24 -o <eth> -j MASQUERADE`。
  - 生产建议：把这些规则放进部署脚本 / systemd `ExecStartPost`（见 `packaging/linux`），或由运维固化；应用本身不擅自改主机防火墙。
- [x] **站点 LAN 网段路由（可配置路由，非全转发）已真机验证（2026-05-25）**：peer 注册时 `routed_subnets` 声明背后 LAN（如 `192.168.20.0/24`），服务端 `KernelWireGuardControl` 自动把网段加入该 peer 的 allowed-ips **并加 `ip route <subnet> dev wg0`**（`wg set` 不像 `wg-quick` 自动加路由，这是关键）。客户端从注册响应的 `allowed_routes` 获知应路由的网段（VPN 子网 + 各站点 LAN），实现分隧道。验证：clientA 经服务端中转 + 站点网关 B 转发，ping 通 B 内网主机 192.168.20.1，0% 丢包。
  - 站点网关侧仍需自行开 `ip_forward` 并把 VPN 流量转发/MASQUERADE 进其本地 LAN。
  - **真实 vpn-cli 客户端端到端复测（2026-05-25）**：网关节点 `vpn-cli login --route 172.31.101.0/24` + daemon up 声明 routed_subnets；
    消费端 vpn-cli 节点重连后 `allowed_routes` 自动含该 LAN，`ping 172.31.101.2`（站点 nginx）0% 丢包、**ttl=62（服务端 −1 + 网关 −1，两跳转发）**、HTTP 200。
    两端均为真实 `vpn-cli` daemon（非裸 wg），靠自动心跳保持 online。网关容器内手动加 `ip_forward=1` + `iptables -t nat -A POSTROUTING -s 10.8.0.0/24 -o eth0 -j MASQUERADE`。
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
