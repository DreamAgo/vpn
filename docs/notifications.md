# 事件通知

易链服务端支持基于事件的多渠道通知。当前已接入的事件：

- 站点网关离线：仅当节点配置了 `routed_subnets`，且心跳超时被标记为离线时触发。
- 站点网关恢复：站点网关从非 online 状态重新心跳成功时触发。
- 飞书审批通过：授权事务成功后，发送到申请人的飞书企业邮箱，包含账号、用户组和北京时间授权截止时间。仅邮件渠道，受通知总开关控制。
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

## 审批通过邮件

授权与邮件任务在同一个数据库事务中提交。按审批实例去重，多用户组只发送一封，历史审批不会补发。
邮件发送独立于审批处理，失败后自动退避重试（最长每小时一次），进程重启后继续处理；失败和成功记录可在通知历史查看。
关闭通知或任务授权已过期时跳过邮件。申请人邮箱不可用时不会转发给管理员。
SMTP 成功后、发送结果落库前若进程异常退出，重试仍可能重复投递；SMTP 不提供严格的跨系统恰好一次保证。
备份格式 v4 包含邮件队列及完成状态，恢复期间与队列 worker 互斥。

### 可编辑模板

通知设置提供审批邮件主题、纯文本正文和示例预览。支持 `{{username}}`、`{{applicant_email}}`、`{{user_groups}}`、`{{expires_at}}`、`{{instance_code}}`，变量内允许首尾空格。
账号、邮箱、用户组、北京时间截止时间、审批实例编号来自授权时保存的上下文；变量值不会再次作为模板执行。
未知变量、未闭合括号、空模板及主题换行会被拒绝。主题最多 200 字，正文最多 20000 字。
保存后待发送及重试邮件使用最新模板；不重发已完成通知。未指定模板的旧版 API 请求保留现有模板。
