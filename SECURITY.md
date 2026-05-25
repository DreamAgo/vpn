# 安全策略

## 报告漏洞

请**不要**通过公开 issue 报告安全漏洞。请通过 GitHub Security Advisory（仓库 Security → Report a vulnerability）私下报告，或邮件联系维护者。我们会在确认后尽快修复并致谢。

## 安全设计要点

- **密码**：argon2id 哈希存储；登录失败指数退避锁定，防暴力破解。
- **令牌**：Access Token 为 JWT RS256（15 分钟有效，仅内存）；Refresh Token 为不透明随机串，以 sha256 存库并可显式撤销（改密/禁用/注销/强制下线即失效）。
- **传输**：生产强制 HTTPS（rustls-acme 自动证书），HTTP 跳转 HTTPS。
- **审计**：所有写操作与登录事件落审计日志，按保留期清理。
- **客户端凭据**：Refresh Token 存系统钥匙串，降级文件用 XSalsa20Poly1305 加密。
- **最小权限**：服务容器非 root 运行，仅授予 `CAP_NET_ADMIN`。

## 依赖审计

```bash
cargo deny check      # 许可证 + 漏洞 + 来源（配置见 deny.toml）
cargo audit           # RustSec 漏洞库
```

CI 在每次 push/PR 运行 `cargo-deny`。发布前应确保无高危项（见 [docs/REAL-HARDWARE-CHECKLIST.md](docs/REAL-HARDWARE-CHECKLIST.md) 第 5 节）。

## 支持范围

仅对最新发布版本提供安全修复。
