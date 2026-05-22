/**
 * 主框架布局（ProLayout 包装）。
 *
 * 设计（来自 UX Spec §Visual Foundation）：
 * - 深色侧边栏（4 项主导航）+ 顶栏（项目名 + 用户菜单）
 * - 主内容区限宽 1600px 居中
 */
import { ProLayout, type ProLayoutProps } from '@ant-design/pro-components';
import { Outlet, useLocation, useNavigate } from 'react-router-dom';
import { DashboardOutlined, UserOutlined, ApiOutlined, FileTextOutlined } from '@ant-design/icons';

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

  return (
    <ProLayout
      title="vpn"
      logo={false}
      layout="mix"
      contentWidth="Fixed"
      location={{ pathname: location.pathname }}
      route={route}
      menuItemRender={(item, dom) =>
        item.path ? (
          <a onClick={() => navigate(item.path!)}>{dom}</a>
        ) : (
          dom
        )
      }
      avatarProps={{
        size: 'small',
        title: 'admin',
      }}
    >
      <Outlet />
    </ProLayout>
  );
}
