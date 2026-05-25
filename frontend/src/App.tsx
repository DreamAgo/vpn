import { useEffect } from 'react';
import { ConfigProvider, App as AntdApp } from 'antd';
import zhCN from 'antd/locale/zh_CN';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { BrowserRouter, Routes, Route, Navigate } from 'react-router-dom';

import { theme } from './theme';
import { AppLayout } from './components/layout/AppLayout';
import { PlaceholderPage } from './components/PlaceholderPage';
import { RequireAuth } from './components/RequireAuth';
import { SetupGate } from './components/SetupGate';
import { LoginPage } from './pages/LoginPage';
import { SetupWizardPage } from './pages/SetupWizardPage';
import { DashboardPage } from './pages/DashboardPage';
import { AccountSettingsPage } from './pages/AccountSettingsPage';
import { UsersPage } from './pages/UsersPage';
import { useAuthStore } from './stores/authStore';

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 30_000,
      refetchOnWindowFocus: false,
      retry: 1,
    },
  },
});

function App() {
  // 启动时从 localStorage 恢复 refresh token / 用户名
  const hydrate = useAuthStore((s) => s.hydrate);
  useEffect(() => {
    hydrate();
  }, [hydrate]);

  return (
    <ConfigProvider locale={zhCN} theme={theme}>
      <AntdApp>
        <QueryClientProvider client={queryClient}>
          <BrowserRouter>
            <Routes>
              {/* 公开页：根据 setup-status 分流 */}
              <Route element={<SetupGate mode="setup" />}>
                <Route path="/setup" element={<SetupWizardPage />} />
              </Route>
              <Route element={<SetupGate mode="login" />}>
                <Route path="/login" element={<LoginPage />} />
              </Route>

              {/* 受保护页：需登录 */}
              <Route element={<RequireAuth />}>
                <Route element={<AppLayout />}>
                  <Route path="/" element={<Navigate to="/dashboard" replace />} />
                  <Route path="/dashboard" element={<DashboardPage />} />
                  <Route path="/account" element={<AccountSettingsPage />} />
                  <Route path="/account/password" element={<AccountSettingsPage />} />
                  <Route path="/users" element={<UsersPage />} />
                  <Route path="/peers" element={<PlaceholderPage name="节点管理" />} />
                  <Route path="/audit-logs" element={<PlaceholderPage name="审计日志" />} />
                </Route>
              </Route>

              <Route path="*" element={<Navigate to="/dashboard" replace />} />
            </Routes>
          </BrowserRouter>
        </QueryClientProvider>
      </AntdApp>
    </ConfigProvider>
  );
}

export default App;
