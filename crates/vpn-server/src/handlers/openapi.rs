//! OpenAPI description for external integrations.

use axum::Json;
use serde_json::{json, Value};

/// GET /api/v1/openapi.json
///
/// Public, read-only OpenAPI 3.1 document. External systems can import this
/// into Swagger UI, Apifox, Postman, or code generators.
pub async fn openapi_json() -> Json<Value> {
    Json(json!({
        "openapi": "3.1.0",
        "info": {
            "title": "易链开放 API",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "易链管理端与客户端对外 REST API。除初始化、登录、刷新、健康检查和 OpenAPI 文档外，接口均使用 Bearer access token 认证。"
        },
        "servers": [
            { "url": "/", "description": "当前服务" }
        ],
        "tags": [
            { "name": "Health", "description": "健康检查" },
            { "name": "Auth", "description": "初始化、登录、刷新、登出、改密" },
            { "name": "System", "description": "服务端状态与 LAN 路由" },
            { "name": "Users", "description": "用户生命周期管理" },
            { "name": "Groups", "description": "用户组与组路由" },
            { "name": "Subnets", "description": "网段目录" },
            { "name": "Peers", "description": "客户端节点注册、心跳、配置下载" },
            { "name": "AdminPeers", "description": "管理员节点治理" },
            { "name": "Audit", "description": "审计日志" },
            { "name": "Backup", "description": "备份与恢复" }
        ],
        "security": [{ "bearerAuth": [] }],
        "paths": {
            "/health": {
                "get": {
                    "tags": ["Health"],
                    "summary": "健康检查",
                    "security": [],
                    "responses": { "200": { "$ref": "#/components/responses/Envelope" } }
                }
            },
            "/api/v1/auth/setup-status": {
                "get": {
                    "tags": ["Auth"],
                    "summary": "查询是否需要首次初始化",
                    "security": [],
                    "responses": { "200": { "$ref": "#/components/responses/Envelope" } }
                }
            },
            "/api/v1/auth/first-time-setup": {
                "post": {
                    "tags": ["Auth"],
                    "summary": "创建首位管理员",
                    "security": [],
                    "requestBody": { "$ref": "#/components/requestBodies/FirstTimeSetup" },
                    "responses": { "200": { "$ref": "#/components/responses/Envelope" } }
                }
            },
            "/api/v1/auth/login": {
                "post": {
                    "tags": ["Auth"],
                    "summary": "登录并获取 access/refresh token",
                    "security": [],
                    "requestBody": { "$ref": "#/components/requestBodies/Login" },
                    "responses": { "200": { "$ref": "#/components/responses/Envelope" } }
                }
            },
            "/api/v1/auth/refresh": {
                "post": {
                    "tags": ["Auth"],
                    "summary": "刷新 access token",
                    "security": [],
                    "requestBody": { "$ref": "#/components/requestBodies/Refresh" },
                    "responses": { "200": { "$ref": "#/components/responses/Envelope" } }
                }
            },
            "/api/v1/auth/logout": {
                "post": {
                    "tags": ["Auth"],
                    "summary": "注销 refresh token",
                    "requestBody": { "$ref": "#/components/requestBodies/Logout" },
                    "responses": { "200": { "$ref": "#/components/responses/Envelope" } }
                }
            },
            "/api/v1/auth/change-password": {
                "post": {
                    "tags": ["Auth"],
                    "summary": "修改当前用户密码",
                    "requestBody": { "$ref": "#/components/requestBodies/ChangePassword" },
                    "responses": { "200": { "$ref": "#/components/responses/Envelope" } }
                }
            },
            "/api/v1/admin/system/info": {
                "get": {
                    "tags": ["System"],
                    "summary": "查询服务端运行信息",
                    "responses": { "200": { "$ref": "#/components/responses/Envelope" } }
                }
            },
            "/api/v1/admin/system/routes": {
                "put": {
                    "tags": ["System"],
                    "summary": "更新服务端 LAN 网段",
                    "requestBody": { "$ref": "#/components/requestBodies/Routes" },
                    "responses": { "200": { "$ref": "#/components/responses/Envelope" } }
                }
            },
            "/api/v1/admin/users": {
                "get": {
                    "tags": ["Users"],
                    "summary": "分页查询用户",
                    "parameters": [
                        { "$ref": "#/components/parameters/Page" },
                        { "$ref": "#/components/parameters/PageSize" },
                        { "name": "search", "in": "query", "schema": { "type": "string" } },
                        { "name": "status", "in": "query", "schema": { "type": "string", "enum": ["active", "disabled"] } }
                    ],
                    "responses": { "200": { "$ref": "#/components/responses/EnvelopePage" } }
                },
                "post": {
                    "tags": ["Users"],
                    "summary": "创建用户",
                    "requestBody": { "$ref": "#/components/requestBodies/CreateUser" },
                    "responses": { "200": { "$ref": "#/components/responses/Envelope" } }
                }
            },
            "/api/v1/admin/users/{id}": {
                "patch": {
                    "tags": ["Users"],
                    "summary": "更新用户状态或终端上限",
                    "parameters": [{ "$ref": "#/components/parameters/Id" }],
                    "requestBody": { "$ref": "#/components/requestBodies/UpdateUser" },
                    "responses": { "200": { "$ref": "#/components/responses/Envelope" } }
                },
                "delete": {
                    "tags": ["Users"],
                    "summary": "删除用户",
                    "parameters": [{ "$ref": "#/components/parameters/Id" }],
                    "responses": { "200": { "$ref": "#/components/responses/Envelope" } }
                }
            },
            "/api/v1/admin/users/{id}/reset-password": {
                "post": {
                    "tags": ["Users"],
                    "summary": "重置用户密码",
                    "parameters": [{ "$ref": "#/components/parameters/Id" }],
                    "responses": { "200": { "$ref": "#/components/responses/Envelope" } }
                }
            },
            "/api/v1/admin/users/{id}/groups": {
                "put": {
                    "tags": ["Users"],
                    "summary": "全量设置用户所属组",
                    "parameters": [{ "$ref": "#/components/parameters/Id" }],
                    "requestBody": { "$ref": "#/components/requestBodies/UserGroups" },
                    "responses": { "200": { "$ref": "#/components/responses/Envelope" } }
                }
            },
            "/api/v1/admin/groups": {
                "get": {
                    "tags": ["Groups"],
                    "summary": "查询用户组",
                    "responses": { "200": { "$ref": "#/components/responses/Envelope" } }
                },
                "post": {
                    "tags": ["Groups"],
                    "summary": "创建用户组",
                    "requestBody": { "$ref": "#/components/requestBodies/Group" },
                    "responses": { "200": { "$ref": "#/components/responses/Envelope" } }
                }
            },
            "/api/v1/admin/groups/{id}": {
                "patch": {
                    "tags": ["Groups"],
                    "summary": "更新用户组",
                    "parameters": [{ "$ref": "#/components/parameters/Id" }],
                    "requestBody": { "$ref": "#/components/requestBodies/Group" },
                    "responses": { "200": { "$ref": "#/components/responses/Envelope" } }
                },
                "delete": {
                    "tags": ["Groups"],
                    "summary": "删除用户组",
                    "parameters": [{ "$ref": "#/components/parameters/Id" }],
                    "responses": { "200": { "$ref": "#/components/responses/Envelope" } }
                }
            },
            "/api/v1/admin/subnets": {
                "get": {
                    "tags": ["Subnets"],
                    "summary": "查询网段目录",
                    "responses": { "200": { "$ref": "#/components/responses/Envelope" } }
                },
                "post": {
                    "tags": ["Subnets"],
                    "summary": "创建网段",
                    "requestBody": { "$ref": "#/components/requestBodies/Subnet" },
                    "responses": { "200": { "$ref": "#/components/responses/Envelope" } }
                }
            },
            "/api/v1/admin/subnets/{id}": {
                "patch": {
                    "tags": ["Subnets"],
                    "summary": "更新网段",
                    "parameters": [{ "$ref": "#/components/parameters/Id" }],
                    "requestBody": { "$ref": "#/components/requestBodies/Subnet" },
                    "responses": { "200": { "$ref": "#/components/responses/Envelope" } }
                },
                "delete": {
                    "tags": ["Subnets"],
                    "summary": "删除网段",
                    "parameters": [{ "$ref": "#/components/parameters/Id" }],
                    "responses": { "200": { "$ref": "#/components/responses/Envelope" } }
                }
            },
            "/api/v1/peers/register": {
                "post": {
                    "tags": ["Peers"],
                    "summary": "客户端注册或续约节点",
                    "requestBody": { "$ref": "#/components/requestBodies/PeerRegister" },
                    "responses": { "200": { "$ref": "#/components/responses/Envelope" } }
                }
            },
            "/api/v1/peers/heartbeat": {
                "post": {
                    "tags": ["Peers"],
                    "summary": "客户端心跳上报",
                    "requestBody": { "$ref": "#/components/requestBodies/PeerHeartbeat" },
                    "responses": { "200": { "$ref": "#/components/responses/Envelope" } }
                }
            },
            "/api/v1/peers/me": {
                "delete": {
                    "tags": ["Peers"],
                    "summary": "注销当前客户端节点",
                    "responses": { "200": { "$ref": "#/components/responses/Envelope" } }
                }
            },
            "/api/v1/peers/me/config": {
                "get": {
                    "tags": ["Peers"],
                    "summary": "下载当前节点 WireGuard 配置",
                    "responses": {
                        "200": {
                            "description": "WireGuard config",
                            "content": { "text/plain": { "schema": { "type": "string" } } }
                        }
                    }
                }
            },
            "/api/v1/admin/peers": {
                "get": {
                    "tags": ["AdminPeers"],
                    "summary": "分页查询节点",
                    "parameters": [
                        { "$ref": "#/components/parameters/Page" },
                        { "$ref": "#/components/parameters/PageSize" },
                        { "name": "search", "in": "query", "schema": { "type": "string" } },
                        { "name": "status", "in": "query", "schema": { "type": "string" } }
                    ],
                    "responses": { "200": { "$ref": "#/components/responses/EnvelopePage" } }
                }
            },
            "/api/v1/admin/peers/{id}": {
                "patch": {
                    "tags": ["AdminPeers"],
                    "summary": "更新节点站点网关网段",
                    "parameters": [{ "$ref": "#/components/parameters/Id" }],
                    "requestBody": { "$ref": "#/components/requestBodies/PeerRoutes" },
                    "responses": { "200": { "$ref": "#/components/responses/Envelope" } }
                },
                "delete": {
                    "tags": ["AdminPeers"],
                    "summary": "强制下线节点",
                    "parameters": [{ "$ref": "#/components/parameters/Id" }],
                    "responses": { "200": { "$ref": "#/components/responses/Envelope" } }
                }
            },
            "/api/v1/admin/peers/{id}/purge": {
                "delete": {
                    "tags": ["AdminPeers"],
                    "summary": "彻底删除节点并回收虚拟 IP",
                    "parameters": [{ "$ref": "#/components/parameters/Id" }],
                    "responses": { "200": { "$ref": "#/components/responses/Envelope" } }
                }
            },
            "/api/v1/admin/peer-events": {
                "get": {
                    "tags": ["AdminPeers"],
                    "summary": "查询节点变更记录",
                    "parameters": [
                        { "name": "peer_id", "in": "query", "schema": { "type": "string" } },
                        { "name": "limit", "in": "query", "schema": { "type": "integer", "default": 20 } }
                    ],
                    "responses": { "200": { "$ref": "#/components/responses/Envelope" } }
                }
            },
            "/api/v1/admin/audit-logs": {
                "get": {
                    "tags": ["Audit"],
                    "summary": "分页查询审计日志",
                    "parameters": [
                        { "$ref": "#/components/parameters/Page" },
                        { "$ref": "#/components/parameters/PageSize" },
                        { "name": "actor_id", "in": "query", "schema": { "type": "string" } },
                        { "name": "action", "in": "query", "schema": { "type": "string" } },
                        { "name": "resource", "in": "query", "schema": { "type": "string" } },
                        { "name": "status_code", "in": "query", "schema": { "type": "integer" } },
                        { "name": "from", "in": "query", "schema": { "type": "integer", "format": "int64" } },
                        { "name": "to", "in": "query", "schema": { "type": "integer", "format": "int64" } }
                    ],
                    "responses": { "200": { "$ref": "#/components/responses/EnvelopePage" } }
                }
            },
            "/api/v1/admin/api-keys": {
                "get": {
                    "tags": ["Auth"],
                    "summary": "查询服务账号 API Key",
                    "responses": { "200": { "$ref": "#/components/responses/Envelope" } }
                },
                "post": {
                    "tags": ["Auth"],
                    "summary": "创建服务账号 API Key",
                    "description": "明文 key 只在创建时返回一次，服务端仅保存哈希。",
                    "requestBody": { "$ref": "#/components/requestBodies/CreateApiKey" },
                    "responses": { "200": { "$ref": "#/components/responses/Envelope" } }
                }
            },
            "/api/v1/admin/api-keys/{id}": {
                "delete": {
                    "tags": ["Auth"],
                    "summary": "吊销服务账号 API Key",
                    "parameters": [{ "$ref": "#/components/parameters/Id" }],
                    "responses": { "200": { "$ref": "#/components/responses/Envelope" } }
                }
            },
            "/api/v1/admin/backup": {
                "get": {
                    "tags": ["Backup"],
                    "summary": "下载系统备份",
                    "responses": {
                        "200": {
                            "description": "Backup JSON",
                            "content": { "application/json": { "schema": { "type": "object" } } }
                        }
                    }
                }
            },
            "/api/v1/admin/backup/restore": {
                "post": {
                    "tags": ["Backup"],
                    "summary": "恢复系统备份",
                    "requestBody": {
                        "required": true,
                        "content": { "application/json": { "schema": { "type": "object" } } }
                    },
                    "responses": { "200": { "$ref": "#/components/responses/Envelope" } }
                }
            }
        },
        "components": {
            "securitySchemes": {
                "bearerAuth": {
                    "type": "http",
                    "scheme": "bearer",
                    "bearerFormat": "JWT"
                }
            },
            "parameters": {
                "Id": {
                    "name": "id",
                    "in": "path",
                    "required": true,
                    "schema": { "type": "string" }
                },
                "Page": {
                    "name": "page",
                    "in": "query",
                    "schema": { "type": "integer", "minimum": 1, "default": 1 }
                },
                "PageSize": {
                    "name": "page_size",
                    "in": "query",
                    "schema": { "type": "integer", "minimum": 1, "maximum": 100, "default": 20 }
                }
            },
            "requestBodies": {
                "Login": { "required": true, "content": { "application/json": { "schema": { "$ref": "#/components/schemas/LoginRequest" } } } },
                "Refresh": { "required": true, "content": { "application/json": { "schema": { "$ref": "#/components/schemas/RefreshRequest" } } } },
                "Logout": { "required": true, "content": { "application/json": { "schema": { "$ref": "#/components/schemas/LogoutRequest" } } } },
                "ChangePassword": { "required": true, "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ChangePasswordRequest" } } } },
                "FirstTimeSetup": { "required": true, "content": { "application/json": { "schema": { "$ref": "#/components/schemas/FirstTimeSetupRequest" } } } },
                "CreateApiKey": { "required": true, "content": { "application/json": { "schema": { "$ref": "#/components/schemas/CreateApiKeyRequest" } } } },
                "CreateUser": { "required": true, "content": { "application/json": { "schema": { "$ref": "#/components/schemas/CreateUserRequest" } } } },
                "UpdateUser": { "required": true, "content": { "application/json": { "schema": { "$ref": "#/components/schemas/UpdateUserRequest" } } } },
                "UserGroups": { "required": true, "content": { "application/json": { "schema": { "type": "object", "required": ["group_ids"], "properties": { "group_ids": { "type": "array", "items": { "type": "string" } } } } } } },
                "Group": { "required": true, "content": { "application/json": { "schema": { "$ref": "#/components/schemas/GroupRequest" } } } },
                "Subnet": { "required": true, "content": { "application/json": { "schema": { "$ref": "#/components/schemas/SubnetRequest" } } } },
                "Routes": { "required": true, "content": { "application/json": { "schema": { "type": "object", "required": ["routes"], "properties": { "routes": { "type": "array", "items": { "type": "string", "example": "192.168.10.0/24" } } } } } } },
                "PeerRoutes": { "required": true, "content": { "application/json": { "schema": { "type": "object", "required": ["routed_subnets"], "properties": { "routed_subnets": { "type": "array", "items": { "type": "string", "example": "192.168.10.0/24" } } } } } } },
                "PeerRegister": { "required": true, "content": { "application/json": { "schema": { "$ref": "#/components/schemas/PeerRegisterRequest" } } } },
                "PeerHeartbeat": { "required": true, "content": { "application/json": { "schema": { "$ref": "#/components/schemas/PeerHeartbeatRequest" } } } }
            },
            "responses": {
                "Envelope": {
                    "description": "统一响应信封",
                    "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiResponse" } } }
                },
                "EnvelopePage": {
                    "description": "统一分页响应信封",
                    "content": { "application/json": { "schema": { "$ref": "#/components/schemas/ApiResponsePage" } } }
                }
            },
            "schemas": {
                "ApiResponse": {
                    "type": "object",
                    "required": ["code", "message", "data", "timestamp", "request_id"],
                    "properties": {
                        "code": { "type": "integer", "description": "业务码，0 表示成功" },
                        "message": { "type": "string" },
                        "data": { "description": "业务数据，错误时为 null" },
                        "timestamp": { "type": "integer", "format": "int64" },
                        "request_id": { "type": "string" }
                    }
                },
                "ApiResponsePage": {
                    "allOf": [{ "$ref": "#/components/schemas/ApiResponse" }]
                },
                "LoginRequest": {
                    "type": "object",
                    "required": ["username", "password"],
                    "properties": {
                        "username": { "type": "string" },
                        "password": { "type": "string", "format": "password" }
                    }
                },
                "RefreshRequest": {
                    "type": "object",
                    "required": ["refresh_token"],
                    "properties": { "refresh_token": { "type": "string" } }
                },
                "LogoutRequest": {
                    "type": "object",
                    "required": ["refresh_token"],
                    "properties": { "refresh_token": { "type": "string" } }
                },
                "ChangePasswordRequest": {
                    "type": "object",
                    "required": ["old_password", "new_password"],
                    "properties": {
                        "old_password": { "type": "string", "format": "password" },
                        "new_password": { "type": "string", "format": "password" }
                    }
                },
                "FirstTimeSetupRequest": {
                    "type": "object",
                    "required": ["username", "email", "password"],
                    "properties": {
                        "username": { "type": "string" },
                        "email": { "type": "string", "format": "email" },
                        "password": { "type": "string", "format": "password" }
                    }
                },
                "CreateApiKeyRequest": {
                    "type": "object",
                    "required": ["name"],
                    "properties": {
                        "name": { "type": "string", "example": "billing-system" },
                        "scopes": {
                            "type": "array",
                            "items": { "type": "string" },
                            "default": ["admin:*"],
                            "description": "权限范围。当前版本存储 scope，认证时按 admin 服务账号处理；细粒度拦截可后续启用。"
                        }
                    }
                },
                "CreateUserRequest": {
                    "type": "object",
                    "required": ["username", "email"],
                    "properties": {
                        "username": { "type": "string" },
                        "email": { "type": "string", "format": "email" },
                        "password": { "type": "string", "format": "password" },
                        "max_devices": { "type": "integer", "minimum": 1, "default": 1 }
                    }
                },
                "UpdateUserRequest": {
                    "type": "object",
                    "properties": {
                        "status": { "type": "string", "enum": ["active", "disabled"] },
                        "max_devices": { "type": "integer", "minimum": 1 }
                    }
                },
                "GroupRequest": {
                    "type": "object",
                    "required": ["name"],
                    "properties": {
                        "name": { "type": "string" },
                        "routed_subnets": { "type": "array", "items": { "type": "string" } }
                    }
                },
                "SubnetRequest": {
                    "type": "object",
                    "required": ["name", "cidr"],
                    "properties": {
                        "name": { "type": "string" },
                        "cidr": { "type": "string", "example": "10.10.0.0/24" },
                        "description": { "type": "string" }
                    }
                },
                "PeerRegisterRequest": {
                    "type": "object",
                    "required": ["device_name", "wg_public_key"],
                    "properties": {
                        "device_name": { "type": "string" },
                        "wg_public_key": { "type": "string" },
                        "os_info": { "type": "string" },
                        "client_version": { "type": "string" }
                    }
                },
                "PeerHeartbeatRequest": {
                    "type": "object",
                    "properties": {
                        "endpoint": { "type": "string" },
                        "wg_public_key": { "type": "string" },
                        "rtt_ms": { "type": "integer", "format": "int64" },
                        "loss_pct": { "type": "number" }
                    }
                }
            }
        }
    }))
}
