/**
 * Story 3.7：创建用户弹窗。
 *
 * - 表单：用户名 / 邮箱 / 初始密码（可一键生成强密码）。
 * - 提交 POST /admin/users；成功后关闭、刷新列表、弹出 ShareLinkModal。
 * - 用户名/邮箱重复（DuplicateResource 3003）：在用户名字段下方报错，保留已填密码。
 */
import { useState } from 'react';
import { Modal, Form, Input, Button, App, Typography, Space } from 'antd';
import { useMutation } from '@tanstack/react-query';

import { usersApi } from '@/services/users';
import { ApiError } from '@/services/http';
import { ErrorCodes } from '@/types/api';
import type { CreateUserResponse } from '@/types/api';
import { codeFontFamily } from '@/theme';
import { ShareLinkModal } from './ShareLinkModal';

const { Text } = Typography;

interface FormValues {
  username: string;
  email: string;
  password: string;
}

interface Props {
  open: boolean;
  onClose: () => void;
  onCreated: () => void;
}

/** 生成 12 位强密码：含大小写字母 + 数字 + 特殊符号，并打乱顺序。 */
function generateStrongPassword(): string {
  const upper = 'ABCDEFGHJKLMNPQRSTUVWXYZ';
  const lower = 'abcdefghijkmnpqrstuvwxyz';
  const digits = '23456789';
  const special = '!@#$%^&*?-_';
  const all = upper + lower + digits + special;

  const rand = (set: string) => set[Math.floor(Math.random() * set.length)];
  const chars = [rand(upper), rand(lower), rand(digits), rand(special)];
  for (let i = chars.length; i < 12; i++) {
    chars.push(rand(all));
  }
  // Fisher-Yates 打乱
  for (let i = chars.length - 1; i > 0; i--) {
    const j = Math.floor(Math.random() * (i + 1));
    [chars[i], chars[j]] = [chars[j], chars[i]];
  }
  return chars.join('');
}

export function CreateUserModal({ open, onClose, onCreated }: Props) {
  const { message } = App.useApp();
  const [form] = Form.useForm<FormValues>();
  const [share, setShare] = useState<{ username: string; password: string } | null>(null);

  const mutation = useMutation({
    mutationFn: (values: FormValues) =>
      usersApi.createUser({
        username: values.username,
        email: values.email,
        password: values.password,
      }),
    onSuccess: (data: CreateUserResponse) => {
      onCreated();
      setShare({ username: data.user.username, password: data.initialPassword });
      form.resetFields();
      onClose();
    },
    onError: (err) => {
      if (err instanceof ApiError) {
        if (err.code === ErrorCodes.DuplicateResource) {
          form.setFields([{ name: 'username', errors: ['用户名/邮箱已存在'] }]);
          return;
        }
        if (err.code === ErrorCodes.PasswordTooWeak) {
          form.setFields([{ name: 'password', errors: [err.message || '密码强度不足'] }]);
          return;
        }
        if (err.code === ErrorCodes.NoAccess || err.code === ErrorCodes.RequireAdmin) {
          message.error('无权限执行该操作');
          return;
        }
        message.error(err.message || '创建用户失败');
        return;
      }
      message.error('网络异常，请稍后再试');
    },
  });

  const handleGenerate = () => {
    form.setFieldValue('password', generateStrongPassword());
    form.setFields([{ name: 'password', errors: [] }]);
  };

  const handleCancel = () => {
    if (mutation.isPending) return;
    form.resetFields();
    onClose();
  };

  return (
    <>
      <Modal
        open={open}
        onCancel={handleCancel}
        title="新建用户"
        maskClosable={false}
        confirmLoading={mutation.isPending}
        okText="创建"
        cancelText="取消"
        onOk={() => form.submit()}
      >
        <Form<FormValues>
          form={form}
          layout="vertical"
          autoComplete="off"
          onFinish={(values) => mutation.mutate(values)}
        >
          <Form.Item
            name="username"
            label="用户名"
            rules={[
              { required: true, message: '请输入用户名' },
              { min: 3, max: 32, message: '用户名长度为 3-32 个字符' },
              {
                pattern: /^[A-Za-z0-9_-]+$/,
                message: '仅允许字母、数字、下划线和连字符',
              },
            ]}
          >
            <Input placeholder="3-32 位，字母/数字/_/-" autoFocus />
          </Form.Item>

          <Form.Item
            name="email"
            label="邮箱"
            rules={[
              { required: true, message: '请输入邮箱' },
              { type: 'email', message: '请输入有效的邮箱地址' },
            ]}
          >
            <Input placeholder="user@example.com" />
          </Form.Item>

          <Form.Item label="初始密码" required style={{ marginBottom: 0 }}>
            <Space.Compact style={{ width: '100%' }}>
              <Form.Item
                name="password"
                noStyle
                rules={[
                  { required: true, message: '请输入初始密码或点击生成' },
                  { min: 8, message: '至少 8 位' },
                ]}
              >
                <Input
                  style={{ fontFamily: codeFontFamily }}
                  placeholder="点击右侧生成 12 位强密码"
                />
              </Form.Item>
              <Button onClick={handleGenerate}>🎲 生成</Button>
            </Space.Compact>
            <Text type="secondary" style={{ display: 'block', marginTop: 4 }}>
              用户首次登录时需修改密码
            </Text>
          </Form.Item>
        </Form>
      </Modal>

      <ShareLinkModal
        open={share !== null}
        onClose={() => setShare(null)}
        username={share?.username ?? ''}
        password={share?.password ?? ''}
      />
    </>
  );
}
