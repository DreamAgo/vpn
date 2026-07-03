import { useMemo } from 'react';
import { ProLayout, type ProLayoutProps } from '@ant-design/pro-components';
import { Dropdown, App, Button, Tooltip } from 'antd';
import { Outlet, useLocation, useNavigate } from 'react-router-dom';
import {
  DashboardOutlined,
  UserOutlined,
  TeamOutlined,
  PartitionOutlined,
  ApiOutlined,
  FileTextOutlined,
  SettingOutlined,
  LogoutOutlined,
  LinkOutlined,
  DatabaseOutlined,
  KeyOutlined,
  BellOutlined,
  MoonOutlined,
  SunOutlined,
} from '@ant-design/icons';

import { useAuthStore } from '@/stores/authStore';
import { EventNotifications } from '@/components/EventNotifications';
import {
  getThemeSurfaces,
  type ThemeMode,
  type ThemePalette,
} from '@/theme';

const adminRoutes: NonNullable<ProLayoutProps['route']>['routes'] = [
  { path: '/dashboard', name: '仪表盘', icon: <DashboardOutlined /> },
  { path: '/users', name: '用户', icon: <UserOutlined /> },
  { path: '/groups', name: '用户组', icon: <TeamOutlined /> },
  { path: '/subnets', name: '网段', icon: <PartitionOutlined /> },
  { path: '/peers', name: '节点', icon: <ApiOutlined /> },
  { path: '/audit-logs', name: '日志', icon: <FileTextOutlined /> },
  { path: '/api-keys', name: 'API Key', icon: <KeyOutlined /> },
  { path: '/notifications', name: '通知设置', icon: <BellOutlined /> },
  { path: '/backup', name: '备份恢复', icon: <DatabaseOutlined /> },
  { path: '/connect', name: '接入指南', icon: <LinkOutlined /> },
];

const userRoutes: NonNullable<ProLayoutProps['route']>['routes'] = [
  { path: '/connect', name: '接入指南', icon: <LinkOutlined /> },
];

function routeForRole(role: string | null): ProLayoutProps['route'] {
  return {
    path: '/',
    routes: role === 'admin' ? adminRoutes : userRoutes,
  };
}

interface AppLayoutProps {
  palette: ThemePalette;
  themeMode: ThemeMode;
  onThemeModeChange: (mode: ThemeMode) => void;
}

function Brand({ collapsed, palette }: { collapsed?: boolean; palette: ThemePalette }) {
  return (
    <div
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: 12,
        minHeight: 44,
        padding: collapsed ? 0 : '0 2px',
      }}
    >
      <span
        style={{
          width: 30,
          height: 30,
          borderRadius: 7,
          background: palette.primary,
          color: '#fff',
          display: 'inline-flex',
          alignItems: 'center',
          justifyContent: 'center',
          fontWeight: 700,
          fontSize: 15,
          flex: 'none',
        }}
      >
        易
      </span>
      {!collapsed && (
        <div style={{ lineHeight: 1.1 }}>
          <div
            style={{
              fontWeight: 700,
              fontSize: 17,
              color: 'var(--ink)',
            }}
          >
            易链
          </div>
          <div
            style={{
              fontSize: 12,
              color: 'var(--ink-soft)',
              marginTop: 4,
            }}
          >
            安全接入管理
          </div>
        </div>
      )}
    </div>
  );
}

function ThemeModeToggle({
  value,
  onChange,
}: {
  value: ThemeMode;
  onChange: (mode: ThemeMode) => void;
}) {
  const next = value === 'dark' ? 'light' : 'dark';
  return (
    <Tooltip title={value === 'dark' ? '切换到浅色模式' : '切换到黑夜模式'}>
      <Button
        type="text"
        aria-label={value === 'dark' ? '切换到浅色模式' : '切换到黑夜模式'}
        icon={value === 'dark' ? <SunOutlined /> : <MoonOutlined />}
        onClick={() => onChange(next)}
        style={{ color: 'var(--ink-soft)' }}
      />
    </Tooltip>
  );
}

export function AppLayout({
  palette,
  themeMode,
  onThemeModeChange,
}: AppLayoutProps) {
  const location = useLocation();
  const navigate = useNavigate();
  const { modal } = App.useApp();
  const username = useAuthStore((s) => s.username);
  const role = useAuthStore((s) => s.role);
  const logout = useAuthStore((s) => s.logout);
  const surfaces = getThemeSurfaces(themeMode);
  const selectedBg = themeMode === 'dark' ? `rgba(${palette.rgb}, 0.16)` : palette.wash;
  const route = useMemo(() => routeForRole(role), [role]);

  const handleLogout = () => {
    modal.confirm({
      title: '退出登录',
      content: '确定要退出当前账号吗？',
      okText: '退出',
      cancelText: '取消',
      onOk: async () => {
        await logout();
        navigate('/login', { replace: true });
      },
    });
  };

  return (
    <ProLayout
      title=""
      logo={false}
      layout="side"
      siderWidth={232}
      fixSiderbar
      fixedHeader
      contentWidth="Fluid"
      location={{ pathname: location.pathname }}
      route={route}
      token={{
        bgLayout: surfaces.bg,
        header: {
          colorBgHeader: surfaces.card,
          colorHeaderTitle: surfaces.ink,
          colorTextRightActionsItem: surfaces.inkSoft,
          heightLayoutHeader: 60,
        },
        sider: {
          colorMenuBackground: surfaces.card,
          colorMenuItemDivider: surfaces.line,
          colorTextMenu: surfaces.inkSoft,
          colorTextMenuSecondary: surfaces.inkFaint,
          colorTextMenuSelected: themeMode === 'dark' ? palette.primary : palette.primaryHover,
          colorBgMenuItemSelected: selectedBg,
          colorBgMenuItemHover: surfaces.menuHover,
          colorTextMenuActive: themeMode === 'dark' ? palette.primary : palette.primaryHover,
          colorTextMenuItemHover: surfaces.ink,
        },
        pageContainer: {
          paddingBlockPageContainerContent: 0,
          paddingInlinePageContainerContent: 0,
        },
      }}
      menuHeaderRender={(_logo, _title, props) => <Brand collapsed={props?.collapsed} palette={palette} />}
      actionsRender={() => [
        role === 'admin' ? <EventNotifications key="events" /> : null,
        <ThemeModeToggle
          key="theme-mode"
          value={themeMode}
          onChange={onThemeModeChange}
        />,
      ]}
      menuItemRender={(item, dom) => (
        <a
          onClick={() => item.path && navigate(item.path)}
          style={{ display: 'flex', alignItems: 'center' }}
        >
          {dom}
        </a>
      )}
      avatarProps={{
        size: 'small',
        title: username ?? 'admin',
        icon: <UserOutlined />,
        render: (_, dom) => (
          <Dropdown
            menu={{
              items: [
                {
                  key: 'account',
                  icon: <SettingOutlined />,
                  label: '账号设置',
                  onClick: () => navigate('/account'),
                },
                { type: 'divider' },
                {
                  key: 'logout',
                  icon: <LogoutOutlined />,
                  label: '退出登录',
                  danger: true,
                  onClick: handleLogout,
                },
              ],
            }}
          >
            {dom}
          </Dropdown>
        ),
      }}
    >
      <main style={{ maxWidth: 1280, margin: '0 auto', width: '100%', padding: '24px 28px 40px' }}>
        <Outlet />
      </main>
    </ProLayout>
  );
}
