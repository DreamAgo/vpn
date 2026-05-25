/**
 * 主框架布局（ProLayout 包装）。
 *
 * 设计（来自 UX Spec §Visual Foundation）：
 * - 深色侧边栏（4 项主导航）+ 顶栏（项目名 + 用户菜单）
 * - 主内容区限宽 1600px 居中
 * - 用户菜单：账号设置 / 退出登录
 */
import { ProLayout, type ProLayoutProps } from '@ant-design/pro-components';
import { Dropdown, App } from 'antd';
import { Outlet, useLocation, useNavigate } from 'react-router-dom';
import {
  DashboardOutlined,
  UserOutlined,
  ApiOutlined,
  FileTextOutlined,
  SettingOutlined,
  LogoutOutlined,
} from '@ant-design/icons';

import { useAuthStore } from '@/stores/authStore';

const route: ProLayoutProps['route'] = {
  path: '/',
  routes: [
    { path: '/dashboard', name: '仪表盘', icon: <DashboardOutlined /> },
    { path: '/users', name: '用户', icon: <UserOutlined /> },
    { path: '/peers', name: '节点', icon: <ApiOutlined /> },
    { path: '/audit-logs', name: '日志', icon: <FileTextOutlined /> },
  ],
};

export function AppLayout() {
  const location = useLocation();
  const navigate = useNavigate();
  const { modal } = App.useApp();
  const username = useAuthStore((s) => s.username);
  const logout = useAuthStore((s) => s.logout);

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
      title="vpn"
      logo={false}
      layout="mix"
      contentWidth="Fixed"
      location={{ pathname: location.pathname }}
      route={route}
      menuItemRender={(item, dom) =>
        item.path ? <a onClick={() => navigate(item.path!)}>{dom}</a> : dom
      }
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
      <Outlet />
    </ProLayout>
  );
}
