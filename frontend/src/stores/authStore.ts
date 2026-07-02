/**
 * 认证全局状态。
 *
 * Access Token 仅存内存（防 XSS）。Refresh Token 暂存 localStorage —
 * Story 4.x 引入 httpOnly cookie 后迁移。
 */
import { create } from 'zustand';
import { setAccessToken, registerRefreshHandler } from '@/services/http';
import { authApi } from '@/services/auth';

const REFRESH_KEY = 'vpn.refreshToken';
const USERNAME_KEY = 'vpn.username';

interface AuthState {
  accessToken: string | null;
  refreshToken: string | null;
  username: string | null;
  mustChangePassword: boolean;
  hydrated: boolean;
  setSession: (args: {
    accessToken: string;
    refreshToken: string;
    username: string;
    mustChangePassword?: boolean;
  }) => void;
  hydrate: () => void;
  refresh: () => Promise<boolean>;
  clearSession: () => void;
  logout: () => Promise<void>;
  setMustChangePassword: (v: boolean) => void;
}

/** 同步读取 localStorage（SPA 端始终可用；出错时回退 null）。 */
function readStored(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

export const useAuthStore = create<AuthState>((set, get) => ({
  accessToken: null,
  // 关键：初始即同步读出 refresh token，使 RequireAuth 首次渲染就能据此静默刷新，
  // 而不是等 App 的 hydrate effect（晚于子组件的路由守卫判定，会误跳登录页）。
  refreshToken: readStored(REFRESH_KEY),
  username: readStored(USERNAME_KEY),
  mustChangePassword: false,
  hydrated: true,

  setSession({ accessToken, refreshToken, username, mustChangePassword = false }) {
    setAccessToken(accessToken);
    localStorage.setItem(REFRESH_KEY, refreshToken);
    localStorage.setItem(USERNAME_KEY, username);
    set({ accessToken, refreshToken, username, mustChangePassword, hydrated: true });
  },

  hydrate() {
    const refreshToken = localStorage.getItem(REFRESH_KEY);
    const username = localStorage.getItem(USERNAME_KEY);
    set({ refreshToken, username, hydrated: true });
  },

  async refresh() {
    const rt = get().refreshToken;
    if (!rt) return false;
    try {
      const res = await authApi.refresh(rt);
      setAccessToken(res.accessToken);
      set({ accessToken: res.accessToken });
      return true;
    } catch {
      get().clearSession();
      return false;
    }
  },

  clearSession() {
    setAccessToken(null);
    localStorage.removeItem(REFRESH_KEY);
    localStorage.removeItem(USERNAME_KEY);
    set({ accessToken: null, refreshToken: null, username: null, mustChangePassword: false });
  },

  async logout() {
    const rt = get().refreshToken;
    if (rt) {
      try {
        await authApi.logout(rt);
      } catch {
        // 忽略：服务端可能已撤销
      }
    }
    get().clearSession();
  },

  setMustChangePassword(v) {
    set({ mustChangePassword: v });
  },
}));

// 注册 401 静默刷新回调（http 拦截器调用，避免循环依赖）。
registerRefreshHandler(async () => {
  const ok = await useAuthStore.getState().refresh();
  return ok ? useAuthStore.getState().accessToken : null;
});
