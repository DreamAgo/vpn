/**
 * 公开页守卫：根据 setup-status 在「首次配置向导」与「登录页」间分流。
 *
 * - needs_setup == true  → 仅允许 /setup
 * - needs_setup == false → 仅允许 /login
 *
 * `mode` 指明当前路由期望的页面；不匹配则重定向。
 */
import { Spin } from 'antd';
import { Navigate, Outlet } from 'react-router-dom';
import { useQuery } from '@tanstack/react-query';

import { authApi } from '@/services/auth';

export function SetupGate({ mode }: { mode: 'setup' | 'login' }) {
  const { data, isLoading, isError } = useQuery({
    queryKey: ['setup-status'],
    queryFn: () => authApi.getSetupStatus(),
    staleTime: 0,
    retry: 1,
  });

  if (isLoading) {
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

  // 接口异常时降级到登录页（避免卡死在空白页）
  const needsSetup = !isError && data?.needsSetup === true;

  if (needsSetup && mode === 'login') {
    return <Navigate to="/setup" replace />;
  }
  if (!needsSetup && mode === 'setup') {
    return <Navigate to="/login" replace />;
  }

  return <Outlet />;
}
