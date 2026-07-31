# 易链对外 API

易链服务端提供标准 REST API，默认前缀为 `/api/v1`。接口契约可通过下面地址获取：

```text
GET /api/v1/openapi.json
```

该 OpenAPI 3.1 文档可导入 Swagger UI、Apifox、Postman 或代码生成工具。

## 认证

除以下接口外，其他接口均使用 Bearer access token：

- `GET /health`
- `GET /api/v1/openapi.json`
- `GET /api/v1/auth/setup-status`
- `POST /api/v1/auth/first-time-setup`
- `POST /api/v1/auth/login`
- `POST /api/v1/auth/refresh`
- `GET /api/v1/auth/feishu/config`
- `POST /api/v1/auth/feishu/start`
- `GET /api/v1/auth/feishu/callback`
- `POST /api/v1/auth/feishu/poll`
- `POST /api/v1/integrations/feishu/approval-options/{source}`（使用请求体中的专用 token）

请求头格式：

```http
Authorization: Bearer <access_token>
```

服务账号也可以使用 API Key 调用管理端 API：

```http
Authorization: Bearer ylk_<id>_<secret>
```

或：

```http
X-API-Key: ylk_<id>_<secret>
```

API Key 只在创建时返回一次明文，服务端只保存哈希。

登录流程：

1. `POST /api/v1/auth/login` 获取 `access_token` 和 `refresh_token`
2. 调用业务接口时使用 `access_token`
3. access token 过期后调用 `POST /api/v1/auth/refresh`
4. 退出登录时调用 `POST /api/v1/auth/logout`

### 飞书 OAuth

服务端配置以下三个环境变量后启用：

- `VPN_FEISHU_APP_ID`
- `VPN_FEISHU_APP_SECRET`（仅存服务端，禁止写入客户端配置或日志）
- `VPN_FEISHU_REDIRECT_URI`（必须为 HTTPS，例如 `https://vpn.example.com/api/v1/auth/feishu/callback`）

飞书开放平台中必须把 `VPN_FEISHU_REDIRECT_URI` 原样加入 OAuth 回调白名单，并为应用开通用户基础信息和邮箱只读权限（`contact:user.base:readonly`、`contact:user.email:readonly`）。

服务端只接受非空 `union_id` 作为稳定身份主键；邮箱会去除首尾空白并转为小写。缺少 `union_id`、邮箱非法或大小写不敏感匹配到多个历史账号时，登录会被拒绝并要求管理员先清理账号数据。

桌面流程：先调用 `config` 探测，再调用 `start` 得到浏览器授权地址和高熵 `poll_token`；浏览器完成固定服务端 `callback` 后，客户端调用 `poll`。pending 时继续短轮询，complete 时只可领取一次现有格式的登录凭证。state、授权码、完成结果、失败结果和过期结果都不可重放，回调 HTML 永不包含 token。

## 响应格式

JSON API 统一返回 `ApiResponse` 信封：

```json
{
  "code": 0,
  "message": "success",
  "data": {},
  "timestamp": 1747000000000,
  "request_id": "..."
}
```

约定：

- `code = 0` 表示业务成功
- `code != 0` 表示业务错误
- `request_id` 会同步写入响应头 `x-request-id`，用于排查日志
- 分页接口的 `data` 为 `{ "items": [], "total": 0, "page": 1, "page_size": 20 }`

飞书审批外部选项接口遵循飞书定义的第三方协议，不使用上述 `ApiResponse`：它返回 `code`、`msg` 和 `data.result`。

## 飞书审批外部选项

用于在飞书审批单选/多选控件中动态展示易链目录数据。当前数据源：

- `subnets`：网段目录，显示为“名称（CIDR）”，选项 ID 使用网段的稳定 ID。
- `user_groups`：用户组目录，显示用户组名称，选项 ID 使用用户组的稳定 ID；`user-groups` 是兼容别名。

配置：

1. 生成至少 32 个字符的独立高熵随机值并设置 `VPN_FEISHU_APPROVAL_OPTIONS_TOKEN`。
2. 在飞书审批后台把请求 URL 填为 `https://<域名>/api/v1/integrations/feishu/approval-options/subnets`。
3. Token 填写与环境变量相同的值；首版不支持可选 Key 加密，因此 Key 必须留空。

用户组控件使用同一 Token，并将 URL 中的数据源替换为 `user_groups`：

```text
https://<域名>/api/v1/integrations/feishu/approval-options/user_groups
```

接口为公网 `POST`，支持飞书的 `query` 与 `page_token` 参数，固定每页最多 50 项。`query` 最长 256 字节，`page_token` 最长 4096 字节。token 缺失或错误时返回 HTTP 401；未配置时返回 HTTP 503；数据源读取超过 2.5 秒时返回 HTTP 504。请求 token 不会写入日志。

请求示例：

```json
{
  "token": "<VPN_FEISHU_APPROVAL_OPTIONS_TOKEN>",
  "query": "办公网",
  "page_token": ""
}
```

成功响应示例：

```json
{
  "code": 0,
  "msg": "success!",
  "data": {
    "result": {
      "options": [{ "id": "<subnet-id>", "value": "@i18n@subnets_<subnet-id>" }],
      "i18nResources": [{
        "locale": "zh_cn",
        "isDefault": true,
        "texts": { "@i18n@subnets_<subnet-id>": "办公网（10.10.0.0/16）" }
      }],
      "hasMore": false
    }
  }
}
```

新增类似目录时，实现并注册一个外部选项 provider 即可复用 token 校验、搜索、签名游标与飞书响应包装。

## 飞书网络授权审批事件

事件回调地址：

```text
https://<域名>/api/v1/integrations/feishu/approval-events
```

运维步骤：

1. 在飞书审批后台发布“网络授权申请”，记下审批定义 `approval_code` 与三个控件的稳定 ID。
2. “网络组”使用上节 `user_groups` 外部选项，首期必须单选；既有 `user-groups` 地址继续兼容。
3. 在应用“事件与回调”中配置上述请求地址、Verification Token 和 Encrypt Key，并添加审批实例状态事件；配置或权限变化后发布应用。
4. 给应用开通读取原生审批实例和联系人基础信息/邮箱所需权限，获取应用 `tenant_access_token`。
5. **按审批定义手动订阅一次**（仅在后台添加事件还不会收到该定义的推送）：

   ```bash
   curl -X POST 'https://open.feishu.cn/open-apis/approval/v4/approvals/<approval_code>/subscribe' \
     -H 'Authorization: Bearer <tenant_access_token>' \
     -H 'Content-Type: application/json; charset=utf-8'
   ```

   返回 `code: 0` 后才表示订阅成功。更换审批定义时需要对新的 `approval_code` 再执行一次；首期不会由服务端自动订阅。
6. 将 `approval_code`、三个控件 ID、Verification Token、Encrypt Key 写入对应 `VPN_FEISHU_APPROVAL_*` 环境变量并重启服务。

普通事件由服务端先校验 5 分钟时间窗和 `X-Lark-Signature`，再 AES-CBC 解密并校验 Verification Token。飞书首次保存回调地址所发的 challenge 可能没有签名头，此时只允许返回已成功解密且 Verification Token 正确的 challenge，不会写入业务数据。只把匹配 `approval_code` 且状态为 `APPROVED` 的普通事件写入 durable inbox，然后快速 ACK；后台 worker 会重新查询审批实例并再次确认状态。授权严格按控件 ID 和外部选项的用户组 ID 解析，不依赖中文标题或显示文案。

每张审批保存独立 `expires_at`。同一审批重推幂等，延期只延长不缩短；人工用户组不会被审批覆盖。新飞书原生账号没有有效审批时只能访问 VPN 基础网段，历史账号继续保持原未分组回退行为。撤回后的追溯撤权、单张审批多组及 userspace/auto 后端 ACL 不在首期范围。

## 主要资源

管理接口：

- 服务账号 API Key：`/api/v1/admin/api-keys`
- 用户：`/api/v1/admin/users`
- 用户组：`/api/v1/admin/groups`
- 网段目录：`/api/v1/admin/subnets`
- 节点治理：`/api/v1/admin/peers`
- 节点变更：`/api/v1/admin/peer-events`
- 服务端状态：`/api/v1/admin/system/info`
- 服务端 LAN：`/api/v1/admin/system/routes`
- 审计日志：`/api/v1/admin/audit-logs`
- 备份恢复：`/api/v1/admin/backup`

客户端接口：

- 注册节点：`POST /api/v1/peers/register`
- 心跳上报：`POST /api/v1/peers/heartbeat`
- 注销当前节点：`DELETE /api/v1/peers/me`
- 下载节点配置：`GET /api/v1/peers/me/config`

## 示例

登录：

```bash
curl -sS http://127.0.0.1:8080/api/v1/auth/login \
  -H 'content-type: application/json' \
  -d '{"username":"admin","password":"password"}'
```

查询用户：

```bash
curl -sS 'http://127.0.0.1:8080/api/v1/admin/users?page=1&page_size=20' \
  -H "authorization: Bearer $ACCESS_TOKEN"
```

导出 OpenAPI：

```bash
curl -sS http://127.0.0.1:8080/api/v1/openapi.json -o yilian-openapi.json
```

创建 API Key：

```bash
curl -sS http://127.0.0.1:8080/api/v1/admin/api-keys \
  -H "authorization: Bearer $ACCESS_TOKEN" \
  -H 'content-type: application/json' \
  -d '{"name":"billing-system","scopes":["admin:*"]}'
```

使用 API Key：

```bash
curl -sS 'http://127.0.0.1:8080/api/v1/admin/users?page=1&page_size=20' \
  -H "authorization: Bearer $YILIAN_API_KEY"
```

为站点网关 peer 设置承载网段（替换语义；传空数组即清空）：

```bash
curl -sS -X PATCH http://127.0.0.1:8080/api/v1/admin/peers/$PEER_ID \
  -H "authorization: Bearer $ACCESS_TOKEN" \
  -H 'content-type: application/json' \
  -d '{"routed_subnets":["192.168.188.0/24"]}'
```

`routed_subnets` 仅能通过该 admin API 修改。客户端注册不接受路由声明；旧客户端继续发送同名字段时，服务端会忽略它并保留现有后台配置。

吊销 API Key：

```bash
curl -sS -X DELETE http://127.0.0.1:8080/api/v1/admin/api-keys/$API_KEY_ID \
  -H "authorization: Bearer $ACCESS_TOKEN"
```

## Scope

当前版本会存储 `scopes`，默认值为 `admin:*`。API Key 通过认证后按服务账号管理员身份访问管理端 API。

建议后续把 scope 落到路由级拦截，例如：

- `users:read`
- `users:write`
- `peers:read`
- `peers:write`
- `audit:read`
- `system:write`
- `admin:*`
