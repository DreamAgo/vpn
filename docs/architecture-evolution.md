# 架构演进计划

本文档记录易链后续架构优化方向，避免功能继续堆在 handler/service 内。

## 1. 通知模块插件化

目标：邮件、Webhook、飞书、钉钉等渠道实现统一接口，通知服务只负责编排规则、去重、历史和重试。

当前状态：已完成第一步，新增 `notification_channels` 模块，邮件与 HTTP 类渠道已实现统一 `Notifier` 接口；`NotificationService` 仍负责配置读取、规则、去重与历史记录。

建议接口：

```rust
#[async_trait::async_trait]
pub trait Notifier: Send + Sync {
    fn channel(&self) -> &'static str;
    async fn send(&self, msg: NotificationMessage) -> Result<()>;
}
```

建议结构：

- `services/notifications/mod.rs`：通知编排、规则、历史、去重
- `services/notifications/channels/email.rs`
- `services/notifications/channels/webhook.rs`
- `services/notifications/channels/feishu.rs`
- `services/notifications/channels/dingtalk.rs`
- `services/notifications/message.rs`：统一消息结构

迁移顺序：

1. 抽出 `NotificationMessage`
2. 为现有邮件和 HTTP 渠道实现 `Notifier`
3. `NotificationService` 改为遍历 `Vec<Box<dyn Notifier>>`
4. 通知历史继续使用 `notification_events`

## 2. 事件总线

目标：业务动作先发布领域事件，再由通知、审计、统计等消费者处理，避免心跳/离线扫描直接调用通知。

当前状态：已完成底座，新增 `domain_events` 表、`DomainEventRepository` 和 `DomainEventService`。网关离线与网关恢复会写入领域事件；通知发送路径暂时保留直接调用，下一步可切到后台消费者。

建议事件：

- `PeerRegistered`
- `PeerHeartbeat`
- `GatewayOffline`
- `GatewayRecovered`
- `PeerRouteChanged`
- `NotificationSent`
- `NotificationFailed`
- `ConfigChanged`

建议表：

```sql
CREATE TABLE domain_events (
    id TEXT PRIMARY KEY,
    event_type TEXT NOT NULL,
    aggregate_type TEXT NOT NULL,
    aggregate_id TEXT NOT NULL,
    payload TEXT NOT NULL,
    status TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    processed_at INTEGER
);
```

迁移顺序：

1. 新增 `DomainEventRepository`
2. 离线扫描和心跳恢复先写 `domain_events`
3. 后台消费者读取 `pending` 事件并触发通知
4. 审计日志也逐步迁到事件消费者

## 3. 配置中心化

目标：把 env、`system_config`、通知配置、服务端路由等统一成配置服务，提供类型化读写、默认值、敏感字段保护。

当前进展：

- 已新增 `ConfigService`，统一封装 `system_config` 的读写入口。
- 已提供字符串、布尔、端口、整数、CSV 列表等类型化读写方法。
- 通知运行时配置已迁入 `ConfigService`，通知服务不再直接读写 `SqliteSystemConfigRepository`。
- `AppState` 已注入 `ConfigService`，后续管理 API 和其它业务服务可以复用同一个配置入口。

建议结构：

- `ConfigService`
- `ConfigKey<T>`
- `ConfigSource`
  - EnvSource
  - DbSource
- `SensitiveString`

配置分类：

- `network.*`：VPN 子网、Endpoint、服务端 LAN
- `notification.*`：通知规则、渠道配置、静默期
- `security.*`：会话、API Key、密码策略
- `backup.*`：备份保留、导出策略

迁移顺序：

1. 保留 `ServerConfig` 负责启动必需项
2. 通知运行时可变配置迁入 `ConfigService`
3. 迁移服务端路由、WG 密钥、网关策略等剩余 `system_config` 读写点
4. 敏感配置如 SMTP 密码、Webhook URL 做加密存储
5. 前端配置页统一调用 `/api/v1/admin/config`

## 4. 前后端类型生成

当前前端 `frontend/src/types/api.ts` 手工维护，容易和 `vpn-api-types` 偏离。

推荐方案：

- 使用 `schemars` 为 Rust DTO 生成 JSON Schema
- 输出到 `target/generated/schema`
- 使用 `json-schema-to-typescript` 生成 TS 类型

建议命令：

```bash
cargo run -p xtask -- generate-schema
npm --prefix frontend run generate:types
```

迁移顺序：

1. 给 `vpn-api-types` DTO 增加 `JsonSchema`
2. 新增 `xtask` 输出 schema
3. 生成 `frontend/src/types/generated.ts`
4. 前端业务类型逐步从 `api.ts` 切换到 generated
5. CI 增加类型生成一致性检查

## 5. 网关拓扑图

已完成第一版：管理端首页基于现有 `peers` 和 `system.serverRoutes` 渲染服务端、接入终端、站点网关、LAN 网段关系。

后续增强：

- 支持点击网关进入节点详情
- 显示网关离线告警、通知状态
- 按用户组过滤拓扑
- 展示 ACL / allowed_routes 实际覆盖关系
