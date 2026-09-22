# DNS 故障恢复验证

本轮修改尚未发布；以下 Windows 场景必须在隔离测试机上验证。单元测试和交叉编译不能证明系统 NRPT 的实际刷新、关机或快速启动行为。

## 修复范围

- Windows GUI 启动、CLI daemon 启动，以及无活动隧道时的连接前，清理本产品旧 DNS 策略，不再依赖注册/API 域名解析成功。
- 转发循环运行后，DNS 维护任务直接从 VPN 源地址向网关查询根 NS；成功才接管，失败恢复系统原 DNS，恢复后重新启用。探测每 5 秒发起一次，最多等待 18 秒；已关闭的维护任务不再启动探测。
- Windows 的 PowerShell 子进程通过 stdin 管道持有规则租约，父进程退出会关闭管道。规则重建为易失性子键，完整重启后不保留。命名互斥体禁止两个实例同时申请规则；启动清理通过 PID 与启动时间跳过仍活动的租约。
- 清理只处理产品标记或本次创建的规则 ID；不删除整个 DnsPolicyConfig，不修改物理网卡 DNS。新规则创建到转为易失性之间有短暂窗口，启动清理仍是必要兜底。
- 系统命令限时 10 秒；停止连接最多等待 45 秒，保留原有清理失败后禁止重连的行为。
- 服务端监听失败重试，不退出 HTTP 控制面；上游错误响应继续尝试备用上游；静态域名非 A 查询不外送；修正零 TTL/无 SOA 负响应缓存，健康查询不走缓存。

## Windows 真机矩阵（待执行）

以管理员运行测试版，保留其他 VPN/企业规则作为隔离检查样本。观察本产品规则：

```powershell
Get-DnsClientNrptRule | Where-Object { $_.Comment -eq 'com.xeflow.yilian.vpn' }
Get-DnsClientNrptPolicy -Effective
Resolve-DnsName example.com
```

| 场景 | 预期 |
| --- | --- |
| 残留旧版持久规则，服务端使用域名，VPN 未连接 | 启动 GUI 即清理旧规则，登录/注册能访问服务端；无需先连通隧道 |
| VPN 握手失败 / 网关 UDP 53 不可达 | 不安装新 NRPT 规则，原有系统解析继续可用 |
| 正常连接且默认上游正常 | 根 NS 探测成功后存在一个产品规则，系统查询可用 |
| 启用后断开 VPN 网络 / 停止 DNS / 后台关闭 DNS | 下一次失败探测后撤销规则；考虑 18 秒探测与 10 秒命令上限，不要求瞬时恢复 |
| DNS 恢复 | 探测成功后重新安装规则 |
| 首个上游超时或返回 SERVFAIL/REFUSED，备用可用 | 备用解析成功；NXDOMAIN 不触发绕过首选上游 |
| 正常断开、退出、连接中取消 | 规则移除、租约子进程退出、旧清理完成后才允许替换隧道 |
| 仅强制结束客户端进程 | 管道 EOF 使租约清理规则，不需要重新连接 |
| 强制结束租约 PowerShell 进程 | DNS 健康任务发现租约退出并清理/重建，不静默维持“已接管”状态 |
| 完整重启、断电后重启 | 已转为易失性的产品规则不恢复；升级前旧规则在首次启动新版时清理 |
| 开启快速启动的关机、休眠唤醒 | 单独检查；易失性注册表键不保证跨休眠消失，必须验证退出清理与启动兜底 |
| 第二个客户端实例启动/连接 | 不删除第一实例活动规则；第二个 DNS 租约被拒绝 |
| PowerShell 启动失败、系统命令超时 | 有限时间报错并回滚；清理不能确认时禁止重连 |
| 企业/其他 VPN 规则存在 | 非本产品规则保持不变 |

全局 DNS 的故障恢复优先保证联网，降级时会使用原系统 DNS，不能作为严格的 DNS 防泄漏策略。由于 DNS 缓存、已有其他更具体的 NRPT 规则、应用自带 DoH，以上检查需结合系统实际生效策略，不能只看浏览器页面或 nslookup。

## 依据

- [Microsoft：Add-DnsClientNrptRule](https://learn.microsoft.com/en-us/powershell/module/dnsclient/add-dnsclientnrptrule)：用 cmdlet 生成规则和值类型，保留产品标记及租约进程身份。
- [Microsoft：RegistryOptions](https://learn.microsoft.com/en-us/dotnet/api/microsoft.win32.registryoptions)：易失性键的生命周期依据；休眠/快速启动需另行验证。

## 本地验证结果（2026-09-21）

- `cargo test --offline -p vpn-platform -p vpn-cli -p vpn-server`：单元、集成与文档测试全部通过，既有真机测试保持忽略。
- `cargo clippy --offline -p vpn-platform -p vpn-cli -p vpn-server --all-targets -- -D warnings`：通过。
- `cargo check --offline -p vpn-platform --tests --target x86_64-pc-windows-gnu`：Windows 平台及测试代码交叉编译通过，未执行 Windows 系统命令。
- `cargo check --offline --manifest-path desktop/src-tauri/Cargo.toml`：macOS 桌面编译通过。
- 根工作区及桌面工作区格式检查、`git diff --check`：通过。
- Windows 真机矩阵未执行；未合入 main、未创建版本标签、未发布或部署。

## Windows 连接耗时优化（2026-09-22，待真机验证）

- 清理 NRPT 前直接读取本地注册表：键不存在或没有子键时不启动 PowerShell；有规则时仍运行原清理脚本，保留产品归属及活动租约检查。
- 重置 VPN 网卡 DNS 前通过 `GetAdaptersAddresses` 读取指定 IPv4 接口索引的 DNS 列表：确认列表为空时不启动 PowerShell。接口未找到、原生查询失败或状态持续变化时回退完整清理。
- 每次重新读取状态，不缓存“已经清理”的结果。没有遗留配置的正常连接路径中，握手前三次 PowerShell 调用均可跳过；恢复遗留配置仍在控制面请求之前执行。
- 日志新增 `dns_command`（操作、跳过/成功/失败、耗时）、`dns_before_connect`、`dns_before_handshake`，并补充 `route_apply` 耗时。结合既有 `tun_open`、`register` 等阶段定位连接慢的原因。
- Windows 每条 PowerShell 命令上限为 30 秒，其他平台命令为 10 秒；这些是超时上限，并非固定等待。

真机对比应在同一机器上分别记录冷启动连接和断开后重连，核对无遗留配置时 `dns_command` 为 `skipped`。另测遗留产品规则、其他产品规则、网卡已有 DNS 和原生检查失败的场景，确保恢复和隔离行为不变。当前不承诺具体秒数，需以慢机器日志实测为准。

原生枚举遵循 [Microsoft GetAdaptersAddresses 文档](https://learn.microsoft.com/en-us/windows/win32/api/iphlpapi/nf-iphlpapi-getadaptersaddresses)，保留 IPv4/IPv6 DNS 地址，采用对齐的缓冲区并处理大小变化。

本次本地验证：平台与 CLI 库测试 183 项通过、2 项既有真机测试忽略；Windows 目标（含新增原生检查测试）通过 Clippy，警告视为错误；macOS 桌面端编译、两工作区格式及差异检查通过。新增 Windows 测试仅完成交叉编译，尚未在 Windows 执行，也未完成连接耗时实测。本次优化未发布。

## macOS 连接耗时优化（2026-09-22，未发布）

- 通过 SystemConfiguration 原生接口读取产品专属动态配置键，缺失时跳过 `scutil`。无遗留配置时，连接前和网卡创建后的两次 `scutil` 清理调用均可跳过；有遗留配置时仍先恢复解析，再请求控制面。
- 使用 `SCDynamicStoreCopyMultiple` 按完整键名查询，空字典表示键不存在，NULL 表示查询失败。查询失败回退原清理流程；每次重新检查，不缓存状态，不修改其他网络服务配置。
- 本机执行平台库测试：36 项通过、2 项既有真机测试忽略，包括原生查询存在/不存在键的只读测试。macOS 平台/CLI Clippy 和桌面编译通过；完整 VPN 连接耗时仍需实测。
- 依据：[Apple SCDynamicStoreCopyMultiple](https://developer.apple.com/documentation/systemconfiguration/scdynamicstorecopymultiple(_:_:_:))。
