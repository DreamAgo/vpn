/**
 * Story 2.10：登录页。
 *
 * - 表单：用户名 + 密码
 * - 提交 → authApi.login → setSession
 * - must_change_password == true → 跳 /account/password
 * - 否则 → 跳 /dashboard
 * - 错误码映射：1001 凭据错误、1003 账号锁定、1004 账号禁用
 */
import { useEffect } from 'react';
import { Form, Input, Button, App } from 'antd';
import { LockOutlined, UserOutlined } from '@ant-design/icons';
import { useNavigate, useLocation } from 'react-router-dom';
import { useMutation } from '@tanstack/react-query';

import { authApi } from '@/services/auth';
import { useAuthStore } from '@/stores/authStore';
import { ApiError } from '@/services/http';
import { ErrorCodes } from '@/types/api';

interface FormValues {
  username: string;
  password: string;
}

export function LoginPage() {
  const navigate = useNavigate();
  const location = useLocation();
  const { message } = App.useApp();
  const setSession = useAuthStore((s) => s.setSession);

  const loggedIn = useAuthStore((s) => !!s.accessToken);
  useEffect(() => {
    if (loggedIn) navigate('/dashboard', { replace: true });
  }, [loggedIn, navigate]);

  const mutation = useMutation({
    mutationFn: (values: FormValues) => authApi.login(values),
    onSuccess: (data, vars) => {
      setSession({
        accessToken: data.accessToken,
        refreshToken: data.refreshToken,
        username: vars.username,
        mustChangePassword: data.mustChangePassword,
      });
      if (data.mustChangePassword) {
        message.warning('首次登录请修改密码');
        navigate('/account/password', { replace: true });
      } else {
        const from = (location.state as { from?: string } | null)?.from ?? '/dashboard';
        navigate(from, { replace: true });
      }
    },
    onError: (err) => {
      if (err instanceof ApiError) {
        switch (err.code) {
          case ErrorCodes.InvalidCredentials:
            message.error('用户名或密码错误');
            return;
          case ErrorCodes.AccountLocked:
            message.error('账号已锁定，请稍后再试');
            return;
          case ErrorCodes.AccountDisabled:
            message.error('账号已被禁用，请联系管理员');
            return;
          case ErrorCodes.RateLimited:
            message.error('登录过于频繁，请稍后再试');
            return;
          default:
            message.error(err.message || '登录失败');
            return;
        }
      }
      message.error('网络异常，请稍后再试');
    },
  });

  return (
    <div
      style={{
        minHeight: '100vh',
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        padding: 24,
        background: 'var(--bg-1)',
      }}
    >
      <div
        style={{
          width: 380,
          background: 'var(--card)',
          border: '1px solid var(--line)',
          borderRadius: 10,
          padding: '34px 34px 30px',
          boxShadow: 'var(--surface-shadow-secondary)',
        }}
      >
        <div style={{ display: 'flex', alignItems: 'center', gap: 10, marginBottom: 6 }}>
          <span
            style={{
              width: 32,
              height: 32,
              borderRadius: 7,
              background: 'var(--accent)',
              color: '#fff',
              display: 'inline-flex',
              alignItems: 'center',
              justifyContent: 'center',
              fontWeight: 700,
            }}
          >
            V
          </span>
          <span style={{ fontWeight: 700, fontSize: 20, color: 'var(--ink)' }}>
            VPN Console
          </span>
        </div>
        <div className="bp-eyebrow" style={{ marginBottom: 26 }}>
          安全接入管理
        </div>

        <Form<FormValues> layout="vertical" autoComplete="off" onFinish={(values) => mutation.mutate(values)}>
          <Form.Item name="username" label="用户名" rules={[{ required: true, message: '请输入用户名' }]}>
            <Input prefix={<UserOutlined />} placeholder="admin" size="large" autoFocus />
          </Form.Item>
          <Form.Item name="password" label="密码" rules={[{ required: true, message: '请输入密码' }]}>
            <Input.Password prefix={<LockOutlined />} placeholder="密码" size="large" />
          </Form.Item>
          <Form.Item style={{ marginBottom: 8, marginTop: 24 }}>
            <Button type="primary" htmlType="submit" block size="large" loading={mutation.isPending}>
              登录
            </Button>
          </Form.Item>
        </Form>

        <div style={{ fontSize: 12, color: 'var(--ink-faint)', marginTop: 18, textAlign: 'center' }}>
          仅限授权管理员访问
        </div>
      </div>
    </div>
  );
}
