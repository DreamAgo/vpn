/**
 * API 响应契约（与 vpn-api-types Rust crate 对齐）。
 *
 * 后端返回 snake_case JSON，axios 拦截器自动转 camelCase。
 * 因此前端 TypeScript 用 camelCase 字段。
 */

export interface ApiResponse<T = unknown> {
  code: number;
  message: string;
  data: T | null;
  timestamp: number;
  requestId: string;
}

export interface Page<T> {
  items: T[];
  total: number;
  page: number;
  pageSize: number;
}

// ===== Auth DTOs (与 vpn-api-types::auth 对齐) =====

export interface SetupStatusResponse {
  needsSetup: boolean;
}

export interface LoginRequest {
  username: string;
  password: string;
}

export interface LoginResponse {
  accessToken: string;
  refreshToken: string;
  accessExpiresIn: number;
  mustChangePassword: boolean;
}

export interface RefreshRequest {
  refreshToken: string;
}

export interface RefreshResponse {
  accessToken: string;
  accessExpiresIn: number;
}

export interface LogoutRequest {
  refreshToken: string;
}

export interface ChangePasswordRequest {
  oldPassword: string;
  newPassword: string;
}

export interface FirstTimeSetupRequest {
  username: string;
  email: string;
  password: string;
}

export interface FirstTimeSetupResponse {
  userId: string;
  accessToken: string;
  refreshToken: string;
}

export interface SystemInfo {
  version: string;
  vpnSubnet: string;
  serverPublicKey: string;
  serverEndpoint: string;
  listenPort: number;
  startedAt: number;
  /** 服务端配置的 LAN 网段（服务端作网关下发给客户端的 allowed_routes）。 */
  serverRoutes?: string[];
}

// ===== User management DTOs (与 vpn-api-types::user 对齐, Epic 3) =====

export interface UserDto {
  id: string;
  username: string;
  email: string;
  role: string; // "admin" | "user"
  status: string; // "active" | "disabled"
  mustChangePassword: boolean;
  lastLoginAt: number | null;
  groupIds: string[]; // 所属用户组 id 列表（可属多个组；未分组为空）
  createdAt: number;
}

// ===== 用户组 DTOs（与 vpn-api-types::group 对齐） =====
export interface UserGroupDto {
  id: string;
  name: string;
  routes: string[]; // 组可路由网段（CIDR）
  memberCount: number;
  createdAt: number;
}

export interface CreateUserGroupRequest {
  name: string;
  routes: string[];
}

export interface UpdateUserGroupRequest {
  name?: string;
  routes?: string[];
}

export interface AssignGroupRequest {
  groupIds: string[]; // 全量覆盖;空数组 → 取消所有分组
}

// ===== 网段目录 DTOs（与 vpn-api-types::subnet 对齐） =====
export interface SubnetDto {
  id: string;
  name: string;
  cidr: string;
  usageCount: number; // 被用户组/节点/服务端路由引用的次数
  createdAt: number;
}

export interface CreateSubnetRequest {
  name: string;
  cidr: string;
}

export interface UpdateSubnetRequest {
  name?: string;
  cidr?: string;
}

export interface CreateUserRequest {
  username: string;
  email: string;
  password?: string;
}

export interface CreateUserResponse {
  user: UserDto;
  initialPassword: string;
}

export interface ListUsersQuery {
  page?: number;
  pageSize?: number;
  search?: string;
  status?: string;
  orderBy?: string;
}

export interface UpdateUserRequest {
  status?: string; // "active" | "disabled"
}

export interface ResetPasswordResponse {
  newPassword: string;
}

// ===== Peer DTOs (与 vpn-api-types::peer 对齐, Epic 4) =====

export interface PeerRegisterRequest {
  wgPublicKey: string;
  deviceName: string;
  osInfo?: string;
  routedSubnets?: string[];
}

export interface PeerRegisterResponse {
  vpnIp: string;
  serverPublicKey: string;
  serverEndpoint: string;
  vpnSubnet: string;
  allowedRoutes?: string[];
}

export interface PeerHeartbeatRequest {
  endpoint?: string;
}

export interface PeerDto {
  id: string;
  userId: string;
  deviceName: string;
  wgPublicKey: string;
  vpnIp: string;
  endpoint: string | null;
  osInfo: string | null;
  lastSeenAt: number | null;
  status: string; // "online" | "offline" | "deleted"
  createdAt: number;
}

// ===== Admin peer / audit DTOs (Epic 5) =====

export interface AdminPeerView {
  id: string;
  userId: string;
  username: string;
  email: string;
  deviceName: string;
  wgPublicKey: string;
  vpnIp: string;
  endpoint: string | null;
  osInfo: string | null;
  lastSeenAt: number | null;
  status: string; // online | offline | deleted | force_removed
  createdAt: number;
  routedSubnets?: string[];
}

export interface UpdatePeerRoutesRequest {
  routedSubnets: string[];
}

export interface AdminPeerQuery {
  page?: number;
  pageSize?: number;
  search?: string;
  status?: string;
}

export interface AuditLogDto {
  id: string;
  userId: string | null;
  username: string | null;
  action: string;
  resource: string;
  ipAddr: string | null;
  userAgent: string | null;
  metadata: string | null;
  statusCode: number | null;
  createdAt: number;
}

export interface AuditLogQuery {
  from?: number;
  to?: number;
  userId?: string;
  username?: string;
  action?: string;
  page?: number;
  pageSize?: number;
}

/** 业务错误码（与 vpn-api-types::error_codes 对齐）。 */
export const ErrorCodes = {
  InvalidCredentials: 1001,
  TokenExpired: 1002,
  AccountLocked: 1003,
  AccountDisabled: 1004,
  PasswordTooWeak: 1005,
  MustChangePassword: 1006,
  MissingAuth: 1007,
  RequireAdmin: 2001,
  NoAccess: 2002,
  UserNotFound: 3001,
  PeerNotFound: 3002,
  DuplicateResource: 3003,
  NotInitialized: 3004,
  AlreadyInitialized: 3005,
  RateLimited: 4001,
  PeerQuotaExceeded: 4002,
  IpPoolExhausted: 4003,
  DatabaseError: 5001,
  WireGuardError: 5002,
  InternalError: 5003,
  ConfigError: 5004,
} as const;
