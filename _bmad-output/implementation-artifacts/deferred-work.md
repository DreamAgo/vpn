# Deferred Work

## Desktop lifecycle and privilege model

- `desktop/src-tauri/src/lib.rs`: `quit_app` 直接调用 `app.exit(0)`，没有等待活动 VPN 任务完成路由清理。该退出行为在本次可观测性改动前已存在；后续应把退出流程改为异步 disconnect → 有界等待 → exit。
- macOS 自提权会以 root 身份重新启动单进程客户端，凭证与应用数据目录可能落到 root 的 home，普通用户不便收集日志。该问题源自既有单进程提权模型；后续应统一原始登录用户的数据目录/ACL，或拆分特权 helper。
- `crates/vpn-cli/src/api.rs` / `cli.rs`: 默认 `reqwest::Client` 未设置请求超时，`run_logout()` 在服务端无响应时可能长期等待，进而延迟主动登出、强制下线或改密后的登录页切换。该网络等待行为在“显示当前登录用户”前已存在；后续应为认证请求增加合理超时，并保持本地凭证清理为 best-effort 优先。

## Peer route state consistency

- `PeerService::force_remove`、`update_peer_routes` 与节点恢复仍缺少统一的状态机锁；admin PATCH 在读取状态后并发 `force_remove`，可能重新配置刚被强制下线的 WireGuard peer。后续应把 peer 状态与路由变更纳入同一事务/锁域。
- `update_peer_routes` 先写数据库再配置 WireGuard；数据面配置失败时会留下数据库与运行时状态不一致。后续应增加补偿回滚或可重放的 reconciliation。
- 对 `force_removed` peer 清空/替换路由时，现有分支不会主动清理历史 OS 路由；后续应统一计算并释放不再被活跃 peer 使用的路由。
- peer 身份仍允许同一账户通过相同 `device_name` 携新公钥匹配旧槽位，这是为客户端重启后密钥变化保留的既有语义，也意味着设备名可被同账户其他客户端冒用。后续应持久化设备密钥或引入管理员批准的设备身份。
