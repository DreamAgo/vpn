# 事件通知

易链服务端支持基于事件的多渠道通知。当前已接入的事件：

- 站点网关离线：仅当节点配置了 `routed_subnets`，且心跳超时被标记为离线时触发。
- 站点网关恢复：站点网关从非 online 状态重新心跳成功时触发。
- 测试邮件：管理员在前端手动触发，用于验证 SMTP 配置。

普通节点离线不会发送邮件，避免通知噪声。

## 通知渠道

当前支持：

- 邮件：SMTP
- 通用 Webhook：结构化 JSON
- 飞书机器人：text 消息
- 钉钉机器人：text 消息

通用 Webhook payload：

```json
{
  "event_type": "gateway_offline",
  "title": "易链通知：站点网关离线 - office-gw",
  "text": "通知正文",
  "metadata": {
    "peer_id": "...",
    "device_name": "office-gw"
  }
}
```

飞书和钉钉使用机器人 Webhook URL，服务端会按各自 text 消息格式发送。

## 邮件配置

通过环境变量启用 SMTP 邮件通知：

```bash
VPN_NOTIFY_EMAIL_ENABLED=true
VPN_SMTP_HOST=smtp.example.com
VPN_SMTP_PORT=587
VPN_SMTP_USERNAME=notice@example.com
VPN_SMTP_PASSWORD=your-password
VPN_NOTIFY_EMAIL_FROM=notice@example.com
VPN_NOTIFY_EMAIL_TO=ops@example.com,admin@example.com
```

说明：

- `VPN_NOTIFY_EMAIL_ENABLED`：设为 `true` 或 `1` 启用
- `VPN_SMTP_HOST`：SMTP 服务器
- `VPN_SMTP_PORT`：默认 `587`
- `VPN_SMTP_USERNAME` / `VPN_SMTP_PASSWORD`：SMTP 认证信息，可按邮件服务商要求配置
- `VPN_NOTIFY_EMAIL_FROM`：发件人
- `VPN_NOTIFY_EMAIL_TO`：收件人，多个地址用英文逗号分隔

配置不完整时，服务端不会发送邮件，只会记录告警日志。

也可以在管理端「通知设置」页面运行时修改配置。运行时配置会写入 `system_config`，优先级高于环境变量，保存后立即生效。

## 触发策略

服务端每 30 秒执行一次离线扫描。节点心跳超过离线阈值后会被标记为 `offline`。

邮件通知只在“在线站点网关首次被标记为离线”的扫描周期发送一次。若该网关恢复在线，之后再次离线，会重新触发通知。

通知规则支持：

- 网关离线通知开关
- 网关恢复通知开关
- 静默期：同一网关同一事件在静默期内只发送一次，默认 30 分钟；静默期内重复事件会写入历史为 `skipped`

## 通知历史

通知发送结果会写入 `notification_events`：

- `sent`：发送成功
- `failed`：发送失败，记录错误原因
- `skipped`：静默期去重跳过

管理端「通知设置」页面会展示最近通知历史。

## 管理 API

- `GET /api/v1/admin/notifications/email`：读取邮件通知配置
- `PUT /api/v1/admin/notifications/email`：保存邮件通知配置、规则和静默期
- `POST /api/v1/admin/notifications/email/test`：发送测试邮件
- `GET /api/v1/admin/notifications/events`：查询通知历史
