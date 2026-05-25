/**
 * axios 实例 + 全局拦截器。
 *
 * 职责：
 * 1. 注入 Authorization Bearer token（来自内存）
 * 2. 请求体 camelCase → snake_case
 * 3. 响应体 snake_case → camelCase
 * 4. 解包 ApiResponse 信封（成功取 data；失败 throw）
 * 5. 401/403 自动处理（清空 token + 跳转 /login）
 */
import axios, { type AxiosResponse, type InternalAxiosRequestConfig } from 'axios';
import { keysToCamel, keysToSnake } from '@/utils/caseConvert';
import type { ApiResponse } from '@/types/api';

// 当前 access token（内存存储，防 XSS；Refresh Token 存 httpOnly cookie 由后端管理）
let accessToken: string | null = null;

export const setAccessToken = (token: string | null) => {
  accessToken = token;
};

export const getAccessToken = (): string | null => accessToken;

// 401 静默刷新回调（由 authStore 注册，避免循环依赖）。
// 返回新的 access token；失败返回 null。
let refreshHandler: (() => Promise<string | null>) | null = null;
export const registerRefreshHandler = (fn: (() => Promise<string | null>) | null) => {
  refreshHandler = fn;
};

export const http = axios.create({
  baseURL: '/api/v1',
  timeout: 30_000,
  withCredentials: true,
  headers: { 'Content-Type': 'application/json' },
});

// 请求拦截：camelCase → snake_case + 注入 token
http.interceptors.request.use((config) => {
  if (accessToken) {
    config.headers.Authorization = `Bearer ${accessToken}`;
  }
  if (config.data && !(config.data instanceof FormData)) {
    config.data = keysToSnake(config.data);
  }
  if (config.params) {
    config.params = keysToSnake(config.params);
  }
  return config;
});

// 响应拦截：解包 ApiResponse + snake_case → camelCase
http.interceptors.response.use(
  (response: AxiosResponse<ApiResponse<unknown>>) => {
    const body = keysToCamel<ApiResponse<unknown>>(response.data);
    if (body.code === 0) {
      // 成功：返回 data 给业务代码
      response.data = body.data as never;
      return response;
    }
    // 业务错误：抛出
    const err = new ApiError(body.code, body.message, body.requestId);
    return Promise.reject(err);
  },
  async (error) => {
    // 网络错误或 HTTP 4xx/5xx
    if (error.response) {
      const original = error.config as
        | (InternalAxiosRequestConfig & { _retried?: boolean; url?: string })
        | undefined;
      const isRefreshCall = original?.url?.includes('/auth/refresh');

      // 401：尝试一次静默刷新后重放原请求
      if (error.response.status === 401 && original && !original._retried && !isRefreshCall) {
        original._retried = true;
        if (refreshHandler) {
          const newToken = await refreshHandler();
          if (newToken) {
            original.headers.Authorization = `Bearer ${newToken}`;
            return http(original);
          }
        }
        accessToken = null;
      }

      const body = keysToCamel<ApiResponse<unknown>>(error.response.data);
      return Promise.reject(
        new ApiError(
          body.code ?? error.response.status,
          body.message ?? error.message,
          body.requestId ?? ''
        )
      );
    }
    return Promise.reject(error);
  }
);

/** 业务错误（含 code + message + requestId 用于排查）。 */
export class ApiError extends Error {
  code: number;
  requestId: string;
  constructor(code: number, message: string, requestId: string) {
    super(message);
    this.code = code;
    this.requestId = requestId;
    this.name = 'ApiError';
  }
}
