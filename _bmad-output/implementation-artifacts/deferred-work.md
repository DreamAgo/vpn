# Deferred Work

## Configuration and notification hardening

- 通知设置 API 仍会在 GET 响应中返回 webhook、飞书机器人和钉钉 URL；这些 URL 往往包含可直接调用的秘密 token。后续应改为与集成设置一致的只写秘密三态与 `*_set` 脱敏视图。
- 通知设置当前按多个 `system_config` key 顺序写入，任一步失败可能留下部分新旧值混合。后续应迁移为单键版本化聚合配置或事务化批量更新。
- `VPN_HTTPS` / `VPN_DOMAIN` 的自动 TLS 配置虽存在于配置和文档，但当前 `main.rs` 实际只启动普通 `axum::serve`，没有接线已有 ACME/TLS 启动实现。后续应完成 TLS listener 接线或修正文档与配置表面，避免误以为源站已启用 HTTPS。
- `main.rs` 只创建 `VPN_DATA_DIR`，不会根据任意 `DATABASE_URL` 自动创建 SQLite 父目录；文档曾宣称会创建。后续应安全解析 file SQLite URL 并只创建本地数据库父目录，或在启动错误中明确要求部署先建目录。

## DNS command lifecycle

- `crates/vpn-platform/src/dns.rs`：既有 `run_commands` 对 `scutil` / PowerShell 等系统命令没有执行超时或取消时终止子进程的保障，命令挂起可能阻塞 DNS 应用、恢复及连接流程。该行为在移除客户端分流前已存在；后续应为整个命令生命周期（含 stdin 写入）增加有界超时、取消清理与故障注入测试。

## Desktop lifecycle and privilege model

- `desktop/src-tauri/src/lib.rs`: `quit_app` 直接调用 `app.exit(0)`，没有等待活动 VPN 任务完成路由清理。该退出行为在本次可观测性改动前已存在；后续应把退出流程改为异步 disconnect → 有界等待 → exit。
- macOS 自提权会以 root 身份重新启动单进程客户端，凭证与应用数据目录可能落到 root 的 home，普通用户不便收集日志。该问题源自既有单进程提权模型；后续应统一原始登录用户的数据目录/ACL，或拆分特权 helper。
- `crates/vpn-cli/src/api.rs` / `cli.rs`: 默认 `reqwest::Client` 未设置请求超时，`run_logout()` 在服务端无响应时可能长期等待，进而延迟主动登出、强制下线或改密后的登录页切换。该网络等待行为在“显示当前登录用户”前已存在；后续应为认证请求增加合理超时，并保持本地凭证清理为 best-effort 优先。

## Peer route state consistency

- `PeerService::force_remove`、`update_peer_routes` 与节点恢复仍缺少统一的状态机锁；admin PATCH 在读取状态后并发 `force_remove`，可能重新配置刚被强制下线的 WireGuard peer。后续应把 peer 状态与路由变更纳入同一事务/锁域。
- `update_peer_routes` 先写数据库再配置 WireGuard；数据面配置失败时会留下数据库与运行时状态不一致。后续应增加补偿回滚或可重放的 reconciliation。
- 对 `force_removed` peer 清空/替换路由时，现有分支不会主动清理历史 OS 路由；后续应统一计算并释放不再被活跃 peer 使用的路由。
- peer 身份仍允许同一账户通过相同 `device_name` 携新公钥匹配旧槽位，这是为客户端重启后密钥变化保留的既有语义，也意味着设备名可被同账户其他客户端冒用。后续应持久化设备密钥或引入管理员批准的设备身份。
- `crates/vpn-cli/src/wg_userspace.rs`：客户端安装授权路由前未为当前 WireGuard/混淆服务端 endpoint 安装宿主旁路。任何包含 endpoint 地址的业务路由都可能把握手 UDP 卷入隧道；该问题在允许 VPN 超网前已可由普通站点路由触发。后续应在加业务路由前固定 endpoint `/32` 到原网关，并在 endpoint 变化时原子更新。
- `crates/vpn-server/src/services/user_group_service.rs`：用户组 `update` 先执行写入，再单独读取记录和成员数；后续读失败时会出现“接口报错但更新已持久化”。该顺序在本次 VPN 超网校验修改前已存在；后续应把 update/get/member_count 收敛到同一事务。

## Authentication session consistency

- `crates/vpn-server/src/services/auth_service.rs`：密码与飞书登录都在检查用户启用状态后再签发并持久化 session；管理员若在该窗口内并发禁用用户，仍可能产生一个随后会在 refresh 时被拒绝、但短期 access token 仍有效的会话。该 TOCTOU 属于既有认证通用问题，后续应把“用户仍启用”校验与 session 创建纳入同一事务，或在每次 access-token 鉴权时校验用户状态。
- `crates/vpn-cli/src/wg_userspace.rs:105`、`:109`：全 workspace 严格 clippy 被两处既有 `manual_inspect` 告警阻断；后续将仅用于记录错误副作用的 `map_err` 改为 `inspect_err`，再恢复 `cargo clippy --workspace --all-targets -- -D warnings` 门禁。

## Feishu callback error UX

- `crates/vpn-server/src/handlers/auth.rs`：`FeishuCallbackQuery.state` 由 Axum 在进入 handler 前强制反序列化；完全缺失或畸形的 `state` 会返回框架通用 400，而非项目的飞书失败提示页。这是自动关闭改动前已存在的 extractor 行为；后续可接收可失败的 query 提取结果并补充无 `state` 的路由级测试，使所有无效回调都呈现统一重试指引。

## Feishu approval network access — later phases

- 审批表单单次选择多个用户组：首版每个审批只接受一个 `user_group.id`，需要多组时分别提交审批；后续扩展多选解析与逐组到期展示。
- 自动调用飞书 `approval/v4/approvals/{approval_code}/subscribe` 并监控订阅状态：现已提供管理员手动订阅按钮与本地成功记录；自动订阅和远端状态监控仍未实现，避免把订阅生命周期并入事件处理事务。
- 管理后台展示审批授权明细、按组到期时间及操作历史：首版已在 SQLite 逐组记录 `expires_at` 和审计数据，但不新增管理 UI。
- ACL 跨后端/跨发行版支持：首版只交付当前生产的 Linux kernel WireGuard + Docker + `CAP_NET_ADMIN`；后续补 userspace/auto、systemd 裸机、iptables-nft/legacy 组合与完整真机矩阵。
- 多服务副本 worker fencing：首版是单 Docker 服务、单 worker；横向扩容前应为 inbox claim 增加 claim token/版本及条件完成写，避免超时旧 worker 覆盖新 worker 状态。
- v1 旧备份生成时尚未导出 `external_identities`，恢复该类历史备份会缺少飞书身份绑定；后续备份工具应区分“字段缺失”和“明确为空”，并提供旧备份迁移/合并策略。

## External options pagination consistency

- `crates/vpn-server/src/services/external_options_service.rs`：现有签名游标使用排序结果的数字 offset；若两个分页请求之间目录项被新增、删除或重命名，后续页可能重复、遗漏或因 offset 越界返回 400。该问题源自既有 `subnets` 通用分页实现，并非本次 `user-groups` provider 引入；后续可改为携带 `(label, id)` 的 keyset 游标，并补并发目录变更测试。
