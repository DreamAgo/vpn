/**
 * 路由守卫：未登录则重定向到 /login。
 *
 * 启动时若内存无 access token 但 localStorage 有 refresh token，
 * 先尝试静默刷新；刷新失败再跳登录。
 */
import { useEffect, useState } from 'react';
import { Spin } from 'antd';
import { Navigate, Outlet, useLocation } from 'react-router-dom';

import { useAuthStore } from '@/stores/authStore';

export function RequireAuth() {
  const location = useLocation();
  const accessToken = useAuthStore((s) => s.accessToken);
  const refreshToken = useAuthStore((s) => s.refreshToken);
  const refresh = useAuthStore((s) => s.refresh);
  const [checking, setChecking] = useState(!accessToken && !!refreshToken);

  useEffect(() => {
    // 初始 checking 状态已涵盖「无需刷新」场景（见 useState 初值），
    // 此处仅在需要静默刷新时异步收尾，避免同步 setState。
    if (!accessToken && refreshToken) {
      let active = true;
      refresh().finally(() => {
        if (active) setChecking(false);
      });
      return () => {
        active = false;
      };
    }
    // 仅在挂载时执行一次静默刷新
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  if (checking) {
    return (
      <div
        style={{
          minHeight: '100vh',
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
        }}
      >
        <Spin size="large" tip="加载中…" />
      </div>
    );
  }

  if (!accessToken) {
    return <Navigate to="/login" replace state={{ from: location.pathname }} />;
  }

  return <Outlet />;
}
