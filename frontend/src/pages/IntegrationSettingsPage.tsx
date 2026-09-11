import { useEffect, useRef, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import {
  Alert,
  Button,
  Card,
  Checkbox,
  Form,
  Input,
  Modal,
  Space,
  Switch,
  Typography,
  App,
} from 'antd';

import { systemApi } from '@/services/auth';
import type {
  IntegrationSettingsView,
  SecretUpdate,
  UpdateIntegrationSettingsRequest,
} from '@/types/api';

interface FormValues {
  loginEnabled: boolean;
  appId?: string;
  redirectUri?: string;
  appSecret?: string;
  clearAppSecret?: boolean;
  approvalEnabled: boolean;
  approvalCode?: string;
  groupControlId?: string;
  expiryControlId?: string;
  reasonControlId?: string;
  verificationToken?: string;
  clearVerificationToken?: boolean;
  encryptKey?: string;
  clearEncryptKey?: boolean;
  optionsToken?: string;
  clearOptionsToken?: boolean;
}

const clean = (value?: string) => value?.trim() || null;
const secret = (value?: string, clear?: boolean): SecretUpdate => ({
  value: clean(value),
  clear: Boolean(clear),
});

function valuesFrom(view: IntegrationSettingsView): FormValues {
  const desired = view.desired;
  return {
    loginEnabled: desired.feishuLogin.enabled,
    appId: desired.feishuLogin.appId ?? undefined,
    redirectUri: desired.feishuLogin.redirectUri ?? undefined,
    approvalEnabled: desired.feishuApproval.enabled,
    approvalCode: desired.feishuApproval.approvalCode ?? undefined,
    groupControlId: desired.feishuApproval.groupControlId ?? undefined,
    expiryControlId: desired.feishuApproval.expiryControlId ?? undefined,
    reasonControlId: desired.feishuApproval.reasonControlId ?? undefined,
  };
}

export function IntegrationSettingsPage() {
  const { message } = App.useApp();
  const queryClient = useQueryClient();
  const [form] = Form.useForm<FormValues>();
  const [dirty, setDirty] = useState(false);
  const dirtyRef = useRef(false);
  const query = useQuery({
    queryKey: ['integration-settings'],
    queryFn: systemApi.getIntegrationSettings,
  });

  useEffect(() => {
    // 后台刷新或迟到响应不得覆盖管理员尚未保存的输入。
    if (query.data && !dirtyRef.current) form.setFieldsValue(valuesFrom(query.data));
  }, [form, query.data]);

  const mutation = useMutation({
    mutationFn: systemApi.updateIntegrationSettings,
    onMutate: async () => {
      await queryClient.cancelQueries({ queryKey: ['integration-settings'] });
    },
    onSuccess: (view) => {
      dirtyRef.current = false;
      setDirty(false);
      form.resetFields();
      form.setFieldsValue(valuesFrom(view));
      queryClient.setQueryData(['integration-settings'], view);
      message.success('集成设置已保存，重启服务端后生效');
    },
    onError: (error: Error) => message.error(error.message || '保存失败'),
  });

  const submit = async (values: FormValues) => {
    const conflicts = [
      [values.appSecret, values.clearAppSecret],
      [values.verificationToken, values.clearVerificationToken],
      [values.encryptKey, values.clearEncryptKey],
      [values.optionsToken, values.clearOptionsToken],
    ].some(([value, clear]) => Boolean(typeof value === 'string' && value.trim() && clear));
    if (conflicts) {
      message.error('同一秘密不能同时填写新值并勾选清除');
      return;
    }
    const clears = [
      values.clearAppSecret,
      values.clearVerificationToken,
      values.clearEncryptKey,
      values.clearOptionsToken,
    ].some(Boolean);
    if (clears) {
      const confirmed = await new Promise<boolean>((resolve) => {
        Modal.confirm({
          title: '确认清除集成密钥？',
          content: '清除可能导致飞书登录、审批或外部选项在重启后不可用。',
          okText: '确认清除并保存',
          okButtonProps: { danger: true },
          cancelText: '取消',
          onOk: () => resolve(true),
          onCancel: () => resolve(false),
        });
      });
      if (!confirmed) return;
    }
    const request: UpdateIntegrationSettingsRequest = {
      feishuLogin: {
        enabled: values.loginEnabled,
        appId: clean(values.appId),
        redirectUri: clean(values.redirectUri),
        appSecret: secret(values.appSecret, values.clearAppSecret),
      },
      feishuApproval: {
        enabled: values.approvalEnabled,
        approvalCode: clean(values.approvalCode),
        groupControlId: clean(values.groupControlId),
        expiryControlId: clean(values.expiryControlId),
        reasonControlId: clean(values.reasonControlId),
        verificationToken: secret(
          values.verificationToken,
          values.clearVerificationToken
        ),
        encryptKey: secret(values.encryptKey, values.clearEncryptKey),
      },
      externalOptions: { token: secret(values.optionsToken, values.clearOptionsToken) },
    };
    mutation.mutate(request);
  };

  const desired = query.data?.desired;
  return (
    <Space direction="vertical" size={16} style={{ width: '100%' }}>
      <div>
        <Typography.Title level={2} style={{ marginBottom: 4 }}>集成设置</Typography.Title>
        <Typography.Text type="secondary">
          管理飞书登录、审批和外部选项。秘密仅显示是否已设置，不会回显。
        </Typography.Text>
      </div>
      {query.data?.restartRequired && (
        <Alert type="warning" showIcon message="配置已保存但尚未应用，请重启服务端" />
      )}
      {query.isError && (
        <Alert type="error" showIcon message="加载集成设置失败" description={(query.error as Error).message} />
      )}
      {query.data && (
        <Alert
          type="info"
          showIcon
          message={`当前运行：飞书登录${query.data.applied.feishuLogin.enabled ? '已启用' : '已关闭'}；飞书审批${query.data.applied.feishuApproval.enabled ? '已启用' : '已关闭'}`}
        />
      )}
      <Form
        form={form}
        disabled={mutation.isPending || query.isLoading || query.isError || !query.data}
        layout="vertical"
        onValuesChange={() => {
          dirtyRef.current = true;
          setDirty(true);
        }}
        onFinish={submit}
        initialValues={{ loginEnabled: false, approvalEnabled: false }}
      >
        <Card title="飞书登录" loading={query.isLoading}>
          <Form.Item name="loginEnabled" label="启用飞书登录" valuePropName="checked">
            <Switch />
          </Form.Item>
          <Form.Item name="appId" label="App ID"><Input autoComplete="off" /></Form.Item>
          <Form.Item
            name="redirectUri"
            label="HTTPS 回调地址"
            help="路径必须为 /api/v1/auth/feishu/callback，不允许 query、fragment 或 URL 凭证。"
          >
            <Input placeholder="https://vpn.example.com/api/v1/auth/feishu/callback" />
          </Form.Item>
          <Form.Item label={`App Secret（${desired?.feishuLogin.appSecretSet ? '已设置' : '未设置'}）`}>
            <Space direction="vertical" style={{ width: '100%' }}>
              <Form.Item name="appSecret" noStyle><Input.Password autoComplete="new-password" placeholder="留空保持现值" /></Form.Item>
              <Form.Item name="clearAppSecret" valuePropName="checked" noStyle><Checkbox>清除现有 App Secret</Checkbox></Form.Item>
            </Space>
          </Form.Item>
        </Card>

        <Card title="飞书审批" style={{ marginTop: 16 }}>
          <Form.Item name="approvalEnabled" label="启用飞书审批" valuePropName="checked"><Switch /></Form.Item>
          <Form.Item name="approvalCode" label="审批 Code"><Input /></Form.Item>
          <Form.Item name="groupControlId" label="用户组控件 ID"><Input /></Form.Item>
          <Form.Item name="expiryControlId" label="到期日控件 ID"><Input /></Form.Item>
          <Form.Item name="reasonControlId" label="申请原因控件 ID"><Input /></Form.Item>
          <SecretField formName="verificationToken" clearName="clearVerificationToken" label="Verification Token" isSet={desired?.feishuApproval.verificationTokenSet} />
          <SecretField formName="encryptKey" clearName="clearEncryptKey" label="Encrypt Key" isSet={desired?.feishuApproval.encryptKeySet} />
        </Card>

        <Card title="审批外部选项" style={{ marginTop: 16 }}>
          <Typography.Paragraph type="secondary">Token 至少 32 个字符。</Typography.Paragraph>
          <SecretField formName="optionsToken" clearName="clearOptionsToken" label="外部选项 Token" isSet={desired?.externalOptions.tokenSet} />
        </Card>

        <Space style={{ marginTop: 16 }}>
          <Button
            type="primary"
            htmlType="submit"
            loading={mutation.isPending}
            disabled={query.isLoading || query.isError || !query.data}
          >保存</Button>
          <Button
            disabled={!dirty}
            onClick={() => {
              dirtyRef.current = false;
              setDirty(false);
              form.resetFields();
              if (query.data) form.setFieldsValue(valuesFrom(query.data));
            }}
          >放弃修改</Button>
          <Typography.Text type="secondary">保存不会自动重启或中断在线会话。</Typography.Text>
        </Space>
      </Form>
    </Space>
  );
}

function SecretField({
  formName,
  clearName,
  label,
  isSet,
}: {
  formName: keyof FormValues;
  clearName: keyof FormValues;
  label: string;
  isSet?: boolean;
}) {
  return (
    <Form.Item label={`${label}（${isSet ? '已设置' : '未设置'}）`}>
      <Space direction="vertical" style={{ width: '100%' }}>
        <Form.Item name={formName} noStyle><Input.Password autoComplete="new-password" placeholder="留空保持现值" /></Form.Item>
        <Form.Item name={clearName} valuePropName="checked" noStyle><Checkbox>清除现有 {label}</Checkbox></Form.Item>
      </Space>
    </Form.Item>
  );
}
