# Deferred Work

## Desktop lifecycle and privilege model

- `desktop/src-tauri/src/lib.rs`: `quit_app` 直接调用 `app.exit(0)`，没有等待活动 VPN 任务完成路由清理。该退出行为在本次可观测性改动前已存在；后续应把退出流程改为异步 disconnect → 有界等待 → exit。
- macOS 自提权会以 root 身份重新启动单进程客户端，凭证与应用数据目录可能落到 root 的 home，普通用户不便收集日志。该问题源自既有单进程提权模型；后续应统一原始登录用户的数据目录/ACL，或拆分特权 helper。
