import { useEffect, useMemo, useState } from 'react';
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
                  <Route path="/" element={<Navigate to="/dashboard" replace />} />
                  <Route path="/dashboard" element={<DashboardPage />} />
                  <Route path="/account" element={<AccountSettingsPage />} />
                  <Route path="/account/password" element={<AccountSettingsPage />} />
                  <Route path="/users" element={<UsersPage />} />
                  <Route path="/groups" element={<GroupsPage />} />
                  <Route path="/subnets" element={<SubnetsPage />} />
                  <Route path="/peers" element={<PeersPage />} />
                  <Route path="/audit-logs" element={<AuditLogsPage />} />
                  <Route path="/connect" element={<ConnectionGuidePage />} />
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
