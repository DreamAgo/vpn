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
