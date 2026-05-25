# 配置参考

`vpn-server` 全部配置通过**环境变量**注入，无需配置文件。下表与 `crates/vpn-server/src/config.rs` 一致。

## 服务端环境变量

| 变量 | 默认值 | 说明 |
|---|---|---|
| `VPN_BIND_ADDR` | `0.0.0.0:8080` | HTTP/HTTPS 监听地址。启用 HTTPS 时通常配合 80/443。 |
| `DATABASE_URL` | `sqlite://./dev.db?mode=rwc` | SQLite 数据库 URL。生产建议指向数据卷，如 `sqlite:///var/lib/vpn-server/vpn.db?mode=rwc`。 |
| `VPN_HTTPS` | `false` | `true`/`1` 启用自动 HTTPS（ACME）。启用时**必须**设置 `VPN_DOMAIN`。 |
| `VPN_DOMAIN` | （无） | 公网域名，用于 ACME 证书申请。`VPN_HTTPS=true` 时必填。 |
| `VPN_DATA_DIR` | `./data` | 数据目录：JWT 私钥、ACME 证书缓存等。生产建议持久化卷。 |
| `VPN_SUBNET` | `10.8.0.0/24` | VPN 虚拟子网（CIDR）。`.1` 预留给服务端，`.2` 起分配给节点。 |
| `VPN_LISTEN_PORT` | `51820` | WireGuard UDP 监听端口。 |
| `VPN_ENDPOINT` | `<VPN_DOMAIN 或 127.0.0.1>:<VPN_LISTEN_PORT>` | 客户端连接服务端用的 `host:port`。多数情况留空由域名推导即可。 |
| `VPN_AUDIT_RETENTION_DAYS` | `180` | 审计日志保留天数，超期由后台任务自动清理。 |
| `RUST_LOG` | `info` | 日志级别（tracing EnvFilter 语法），如 `vpn_server=debug,info`。 |

## 启动校验

- `VPN_HTTPS=true` 但缺少 `VPN_DOMAIN` → 启动失败并报错。
- 数据目录与数据库父目录会在启动时自动创建。

## 端口

| 端口 | 协议 | 用途 |
|---|---|---|
| `VPN_BIND_ADDR` 端口（默认 8080） | TCP | HTTP API + Web 后台 |
| 80 / 443 | TCP | 启用 HTTPS 时：80 用于 ACME HTTP-01 + 跳转，443 用于 Web/API |
| `VPN_LISTEN_PORT`（默认 51820） | UDP | WireGuard 数据平面 |

## 最小生产配置示例

```bash
VPN_HTTPS=true
VPN_DOMAIN=vpn.example.com
DATABASE_URL=sqlite:///var/lib/vpn-server/vpn.db?mode=rwc
VPN_DATA_DIR=/var/lib/vpn-server
VPN_SUBNET=10.8.0.0/24
VPN_LISTEN_PORT=51820
RUST_LOG=info
```
