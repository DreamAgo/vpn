import { useEffect, useMemo, useState, type ReactNode } from 'react';
import { ConfigProvider, App as AntdApp } from 'antd';
import zhCN from 'antd/locale/zh_CN';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { BrowserRouter, Routes, Route, Navigate } from 'react-router-dom';

import {
  applyThemeAppearance,
  createAppTheme,
  DEFAULT_THEME_MODE,
  defaultThemePalette,
  isThemeMode,
  type ThemeMode,
} from './theme';
import { AppLayout } from './components/layout/AppLayout';
import { RequireAuth } from './components/RequireAuth';
import { SetupGate } from './components/SetupGate';
import { LoginPage } from './pages/LoginPage';
import { SetupWizardPage } from './pages/SetupWizardPage';
import { DashboardPage } from './pages/DashboardPage';
import { AccountSettingsPage } from './pages/AccountSettingsPage';
import { UsersPage } from './pages/UsersPage';
import { GroupsPage } from './pages/GroupsPage';
import { SubnetsPage } from './pages/SubnetsPage';
import { PeersPage } from './pages/PeersPage';
import { AuditLogsPage } from './pages/AuditLogsPage';
import { ConnectionGuidePage } from './pages/ConnectionGuidePage';
import { BackupPage } from './pages/BackupPage';
import { ApiKeysPage } from './pages/ApiKeysPage';
import { NotificationSettingsPage } from './pages/NotificationSettingsPage';
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

const THEME_MODE_STORAGE_KEY = 'vpn-console-theme-mode';
const ADMIN_HOME = '/dashboard';
const USER_HOME = '/connect';

function DefaultRedirect() {
  const role = useAuthStore((s) => s.role);
  return <Navigate to={role === 'admin' ? ADMIN_HOME : USER_HOME} replace />;
}

function RequireAdminRoute({ children }: { children: ReactNode }) {
  const role = useAuthStore((s) => s.role);
  if (role !== 'admin') return <Navigate to={USER_HOME} replace />;
  return children;
}

function getInitialThemeMode(): ThemeMode {
  if (typeof window === 'undefined') return DEFAULT_THEME_MODE;
  const stored = window.localStorage.getItem(THEME_MODE_STORAGE_KEY);
  return isThemeMode(stored) ? stored : DEFAULT_THEME_MODE;
}

function App() {
  const [themeMode, setThemeMode] = useState<ThemeMode>(getInitialThemeMode);
  const palette = useMemo(() => defaultThemePalette, []);
  const appTheme = useMemo(() => createAppTheme(palette, themeMode), [palette, themeMode]);

  // 启动时从 localStorage 恢复 refresh token / 用户名
  const hydrate = useAuthStore((s) => s.hydrate);
  useEffect(() => {
    hydrate();
  }, [hydrate]);

  useEffect(() => {
    applyThemeAppearance(palette, themeMode);
    window.localStorage.setItem(THEME_MODE_STORAGE_KEY, themeMode);
  }, [palette, themeMode]);

  return (
    <ConfigProvider locale={zhCN} theme={appTheme}>
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
                <Route
                  element={
                    <AppLayout
                      palette={palette}
                      themeMode={themeMode}
                      onThemeModeChange={setThemeMode}
                    />
                  }
                >
                  <Route path="/" element={<DefaultRedirect />} />
                  <Route
                    path="/dashboard"
                    element={
                      <RequireAdminRoute>
                        <DashboardPage />
                      </RequireAdminRoute>
                    }
                  />
                  <Route path="/account" element={<AccountSettingsPage />} />
                  <Route path="/account/password" element={<AccountSettingsPage />} />
                  <Route
                    path="/users"
                    element={
                      <RequireAdminRoute>
                        <UsersPage />
                      </RequireAdminRoute>
                    }
                  />
                  <Route
                    path="/groups"
                    element={
                      <RequireAdminRoute>
                        <GroupsPage />
                      </RequireAdminRoute>
                    }
                  />
                  <Route
                    path="/subnets"
                    element={
                      <RequireAdminRoute>
                        <SubnetsPage />
                      </RequireAdminRoute>
                    }
                  />
                  <Route
                    path="/peers"
                    element={
                      <RequireAdminRoute>
                        <PeersPage />
                      </RequireAdminRoute>
                    }
                  />
                  <Route
                    path="/audit-logs"
                    element={
                      <RequireAdminRoute>
                        <AuditLogsPage />
                      </RequireAdminRoute>
                    }
                  />
                  <Route
                    path="/api-keys"
                    element={
                      <RequireAdminRoute>
                        <ApiKeysPage />
                      </RequireAdminRoute>
                    }
                  />
                  <Route
                    path="/backup"
                    element={
                      <RequireAdminRoute>
                        <BackupPage />
                      </RequireAdminRoute>
                    }
                  />
                  <Route
                    path="/notifications"
                    element={
                      <RequireAdminRoute>
                        <NotificationSettingsPage />
                      </RequireAdminRoute>
                    }
                  />
                  <Route path="/connect" element={<ConnectionGuidePage />} />
                </Route>
              </Route>

              <Route path="*" element={<DefaultRedirect />} />
            </Routes>
          </BrowserRouter>
        </QueryClientProvider>
      </AntdApp>
    </ConfigProvider>
  );
}

export default App;
