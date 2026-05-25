/**
 * Story 2.11：首次配置向导（仅当 needs_setup == true 时显示）。
 *
 * 单页表单：用户名 + 邮箱 + 密码 + 确认密码。
 * 提交 → POST /auth/first-time-setup → 自动登录 → 跳 /dashboard。
 */
import { Card, Form, Input, Button, Typography, Steps, App } from 'antd';
import { useNavigate } from 'react-router-dom';
import { useMutation } from '@tanstack/react-query';

import { authApi } from '@/services/auth';
import { useAuthStore } from '@/stores/authStore';
import { ApiError } from '@/services/http';
import { ErrorCodes } from '@/types/api';

const { Title, Paragraph } = Typography;

interface FormValues {
  username: string;
  email: string;
  password: string;
  confirmPassword: string;
}

export function SetupWizardPage() {
  const navigate = useNavigate();
  const { message } = App.useApp();
  const setSession = useAuthStore((s) => s.setSession);

  const mutation = useMutation({
    mutationFn: (values: FormValues) =>
      authApi.firstTimeSetup({
        username: values.username,
        email: values.email,
        password: values.password,
      }),
    onSuccess: (data, vars) => {
      setSession({
        accessToken: data.accessToken,
        refreshToken: data.refreshToken,
        username: vars.username,
        mustChangePassword: false,
      });
      message.success('初始化完成，已自动登录');
      navigate('/dashboard', { replace: true });
    },
    onError: (err) => {
      if (err instanceof ApiError) {
        if (err.code === ErrorCodes.AlreadyInitialized) {
          message.error('系统已初始化，请直接登录');
          navigate('/login', { replace: true });
          return;
        }
        if (err.code === ErrorCodes.PasswordTooWeak) {
          message.error(`密码强度不足：${err.message}`);
          return;
        }
        message.error(err.message || '初始化失败');
        return;
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
        background: 'linear-gradient(135deg, #1f2a4a 0%, #2F54EB 100%)',
        padding: 24,
      }}
    >
      <Card style={{ width: 560, boxShadow: '0 8px 32px rgba(0,0,0,0.15)' }}>
        <Title level={3} style={{ marginBottom: 8 }}>
          欢迎使用 vpn — 首次配置
        </Title>
        <Paragraph type="secondary" style={{ marginBottom: 16 }}>
          创建首位管理员账号。完成后即可登录后台开始管理用户与节点。
        </Paragraph>
        <Steps
          size="small"
          current={0}
          style={{ marginBottom: 24 }}
          items={[
            { title: '创建管理员' },
            { title: '完成', description: '登录后台' },
          ]}
        />
        <Form<FormValues>
          layout="vertical"
          autoComplete="off"
          onFinish={(values) => mutation.mutate(values)}
        >
          <Form.Item
            name="username"
            label="管理员用户名"
            rules={[
              { required: true, message: '请输入用户名' },
              { min: 3, max: 32, message: '长度 3-32 个字符' },
              { pattern: /^[a-zA-Z0-9_-]+$/, message: '仅支持字母、数字、下划线、横线' },
            ]}
          >
            <Input placeholder="admin" autoFocus />
          </Form.Item>
          <Form.Item
            name="email"
            label="邮箱"
            rules={[
              { required: true, message: '请输入邮箱' },
              { type: 'email', message: '邮箱格式不正确' },
            ]}
          >
            <Input placeholder="admin@example.com" />
          </Form.Item>
          <Form.Item
            name="password"
            label="密码"
            rules={[
              { required: true, message: '请输入密码' },
              { min: 8, message: '至少 8 位' },
              { pattern: /(?=.*[A-Za-z])(?=.*\d)/, message: '需同时包含字母和数字' },
            ]}
            hasFeedback
          >
            <Input.Password placeholder="≥ 8 位，含字母和数字" />
          </Form.Item>
          <Form.Item
            name="confirmPassword"
            label="确认密码"
            dependencies={['password']}
            hasFeedback
            rules={[
              { required: true, message: '请再次输入密码' },
              ({ getFieldValue }) => ({
                validator(_, value) {
                  if (!value || getFieldValue('password') === value) return Promise.resolve();
                  return Promise.reject(new Error('两次密码不一致'));
                },
              }),
            ]}
          >
            <Input.Password />
          </Form.Item>
          <Form.Item>
            <Button
              type="primary"
              htmlType="submit"
              block
              size="large"
              loading={mutation.isPending}
            >
              创建管理员账号
            </Button>
          </Form.Item>
        </Form>
      </Card>
    </div>
  );
}
