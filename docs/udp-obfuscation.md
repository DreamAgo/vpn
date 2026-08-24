# WireGuard UDP 混淆传输

服务端可启用 Rust 原生 `obfs-v1`，用于隐藏公网 WireGuard 固定握手特征。它不是新的
VPN 身份认证层：节点认证、隧道加密和数据重放保护仍由 WireGuard 完成。

## 部署

生成 32 字节共享密钥（输出为标准 Base64）：

```bash
openssl rand -base64 32
```

将结果保存到部署环境的 `VPN_OBFS_PSK`，不要写入 Git、日志或镜像。Docker Compose
默认仅发布 `47358/udp`，内部 WireGuard `51820/udp` 不应在宿主机或云安全组开放。

```bash
export VPN_OBFS_PSK='<上一步结果>'
docker compose -f docker/docker-compose.yml up -d
```

控制面必须启用 HTTPS。服务端会在节点注册响应中下发模式、endpoint、路径 MTU 和
PSK；若控制面是 HTTP，服务端会拒绝启动混淆功能。旧客户端不声明 `obfs-v1`
能力时注册会被拒绝，不会静默降级为原生 WireGuard。

## 配置

| 环境变量 | 默认值 | 说明 |
|---|---|---|
| `VPN_OBFS_ENABLED` | `false` | 启用混淆并强制新客户端使用 |
| `VPN_OBFS_PSK` | 无 | 32 字节标准 Base64，共享密钥 |
| `VPN_OBFS_MODE` | `low-overhead-v1` | `low-overhead-v1` 或 `paranoid-v1` |
| `VPN_OBFS_BIND_ADDR` | `0.0.0.0:47358` | 公网 UDP 监听地址 |
| `VPN_OBFS_ENDPOINT` | `<VPN_DOMAIN>:47358` | 下发给客户端的公网地址 |
| `VPN_OBFS_PATH_MTU` | `1500` | 路径 MTU，范围 576–9000 |

内网 split-horizon 场景在客户端设置 `VPN_OBFS_ENDPOINT_OVERRIDE=内网地址:47358`。
原生 WireGuard 的 `VPN_ENDPOINT_OVERRIDE` 不作用于混淆传输，避免把混淆报文误发到
51820 端口。

低开销模式在路径允许时保持客户端 TUN MTU 1420，较小路径会自动下调，数据包大小不增加；握手包使用随机 nonce、
时间戳、AEAD 和随机填充。全填充模式对每个数据包做 AEAD，并填充到路径 UDP 上限，
客户端会按 16 字节边界自动降低 TUN MTU。

服务端按公网来源 `IP:port` 建立独立内部 WireGuard UDP socket。新来源在成功解码、
认证并完成 WireGuard 格式校验后才会创建会话；默认上限 4096 个，每个来源 IP 每分钟
最多创建 20 个，空闲 180 秒回收。网络切换会产生新会话，WireGuard 自行完成 endpoint
漫游。

## 时钟与诊断

握手允许客户端与服务端时钟相差 15 秒，已认证握手 nonce 保留 30 秒防重放。请在两端
启用 NTP。日志使用以下阶段字段，且不会记录 PSK 或载荷：

- `obfs_listen`：监听与模式；
- `obfs_session`：会话创建和回收；
- `obfs_drop_summary`：非法、重放、时钟偏差、限速和容量丢弃聚合计数；
- `obfs_client`：客户端模式和动态 MTU。

若出现 `clock` 丢弃，先检查两端系统时间；若出现 `rate_limited` 或 `capacity`，检查是否
有扫描/洪泛流量。配置错误或混淆传输失败时客户端不会回退到原生 WireGuard。
