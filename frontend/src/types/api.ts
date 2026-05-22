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
