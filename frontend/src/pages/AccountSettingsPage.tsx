/**
 * Story 2.13：账号设置页（修改密码）。
 *
 * 同一页面服务两种场景：
 * - 普通修改密码（从用户菜单进入 /account）
 * - 首次登录强制改密（must_change_password，进入 /account/password）
 *
 * 改密成功 → 后端撤销所有 session → 清空本地会话 → 跳登录页重新登录。
 */
import { useEffect } from 'react';
import { Card, Form, Input, Button, Typography, App, Alert, Descriptions } from 'antd';
import { useNavigate, useLocation } from 'react-router-dom';
import { useMutation } from '@tanstack/react-query';

import { authApi } from '@/services/auth';
import { useAuthStore } from '@/stores/authStore';
import { ApiError } from '@/services/http';
import { ErrorCodes } from '@/types/api';

const { Title } = Typography;

interface FormValues {
  oldPassword: string;
  newPassword: string;
  confirmPassword: string;
}

export function AccountSettingsPage() {
  const navigate = useNavigate();
  const location = useLocation();
  const { message } = App.useApp();
  const username = useAuthStore((s) => s.username);
  const role = useAuthStore((s) => s.role);
  const mustChangePassword = useAuthStore((s) => s.mustChangePassword);
  const clearSession = useAuthStore((s) => s.clearSession);

  // 强制改密场景：拦截，提示用户必须先改密
  const forced = location.pathname.endsWith('/password') || mustChangePassword;

  useEffect(() => {
    if (mustChangePassword && !location.pathname.endsWith('/password')) {
      navigate('/account/password', { replace: true });
    }
  }, [mustChangePassword, location.pathname, navigate]);

  const mutation = useMutation({
    mutationFn: (values: FormValues) =>
      authApi.changePassword({
        oldPassword: values.oldPassword,
        newPassword: values.newPassword,
      }),
    onSuccess: () => {
      message.success('密码已修改，请使用新密码重新登录');
      clearSession();
      navigate('/login', { replace: true });
    },
    onError: (err) => {
      if (err instanceof ApiError) {
        if (err.code === ErrorCodes.InvalidCredentials) {
          message.error('旧密码错误');
          return;
        }
        if (err.code === ErrorCodes.PasswordTooWeak) {
          message.error(`新密码强度不足：${err.message}`);
          return;
        }
        message.error(err.message || '修改密码失败');
        return;
      }
      message.error('网络异常，请稍后再试');
    },
  });

  return (
    <div style={{ maxWidth: 640 }}>
      <Title level={4} style={{ marginBottom: 16 }}>
        账号设置
      </Title>

      <Card title="账号信息" style={{ marginBottom: 16 }}>
        <Descriptions column={1} size="small">
          <Descriptions.Item label="用户名">{username ?? '—'}</Descriptions.Item>
          <Descriptions.Item label="角色">{role === 'admin' ? '管理员' : '普通用户'}</Descriptions.Item>
        </Descriptions>
      </Card>

      <Card title="修改密码">
        {forced && (
          <Alert
            type="warning"
            showIcon
            style={{ marginBottom: 16 }}
            message="首次登录需修改初始密码"
            description="为保障安全，请设置新密码后重新登录。"
          />
        )}
        <Form<FormValues>
          layout="vertical"
          autoComplete="off"
          onFinish={(values) => mutation.mutate(values)}
        >
          <Form.Item
            name="oldPassword"
            label="当前密码"
            rules={[{ required: true, message: '请输入当前密码' }]}
          >
            <Input.Password placeholder="当前密码" autoFocus />
          </Form.Item>
          <Form.Item
            name="newPassword"
            label="新密码"
            rules={[
              { required: true, message: '请输入新密码' },
              { min: 8, message: '至少 8 位' },
              { pattern: /(?=.*[A-Za-z])(?=.*\d)/, message: '需同时包含字母和数字' },
            ]}
            hasFeedback
          >
            <Input.Password placeholder="≥ 8 位，含字母和数字" />
          </Form.Item>
          <Form.Item
            name="confirmPassword"
            label="确认新密码"
            dependencies={['newPassword']}
            hasFeedback
            rules={[
              { required: true, message: '请再次输入新密码' },
              ({ getFieldValue }) => ({
                validator(_, value) {
                  if (!value || getFieldValue('newPassword') === value) return Promise.resolve();
                  return Promise.reject(new Error('两次密码不一致'));
                },
              }),
            ]}
          >
            <Input.Password />
          </Form.Item>
          <Form.Item>
            <Button type="primary" htmlType="submit" loading={mutation.isPending}>
              修改密码
            </Button>
          </Form.Item>
        </Form>
      </Card>
    </div>
  );
}
