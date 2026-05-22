import { ConfigProvider, App as AntdApp } from 'antd';
import zhCN from 'antd/locale/zh_CN';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { BrowserRouter, Routes, Route, Navigate } from 'react-router-dom';

import { theme } from './theme';
import { AppLayout } from './components/layout/AppLayout';
import { PlaceholderPage } from './components/PlaceholderPage';

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
  return (
    <ConfigProvider locale={zhCN} theme={theme}>
      <AntdApp>
        <QueryClientProvider client={queryClient}>
          <BrowserRouter>
            <Routes>
              <Route element={<AppLayout />}>
                <Route path="/" element={<Navigate to="/dashboard" replace />} />
                <Route path="/dashboard" element={<PlaceholderPage name="仪表盘" />} />
                <Route path="/users" element={<PlaceholderPage name="用户管理" />} />
                <Route path="/peers" element={<PlaceholderPage name="节点管理" />} />
                <Route path="/audit-logs" element={<PlaceholderPage name="审计日志" />} />
              </Route>
            </Routes>
          </BrowserRouter>
        </QueryClientProvider>
      </AntdApp>
    </ConfigProvider>
  );
}

export default App;
