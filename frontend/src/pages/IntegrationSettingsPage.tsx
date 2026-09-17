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
  maxDevicesControlId?: string;
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
    maxDevicesControlId: desired.feishuApproval.maxDevicesControlId ?? undefined,
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
  const subscriptionQueryKey = [
    'feishu-approval-subscription',
    query.data?.applied.feishuLogin.appId,
    query.data?.applied.feishuApproval.approvalCode,
    query.data?.applied.feishuLogin.enabled,
    query.data?.applied.feishuApproval.enabled,
    query.data?.restartRequired,
  ];
  const subscriptionQuery = useQuery({
    queryKey: subscriptionQueryKey,
    queryFn: systemApi.getFeishuApprovalSubscription,
    enabled: Boolean(query.data),
  });
  const subscriptionMutation = useMutation({
    mutationFn: systemApi.subscribeFeishuApproval,
    onMutate: async () => {
      await queryClient.cancelQueries({ queryKey: ['feishu-approval-subscription'] });
    },
    onSuccess: (view) => {
      queryClient.setQueryData(subscriptionQueryKey, view);
      message.success('审批事件订阅成功，已记录执行时间');
    },
    onError: (error: Error) => message.error(error.message || '审批事件订阅失败'),
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: ['feishu-approval-subscription'] });
    },
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
      void queryClient.invalidateQueries({ queryKey: ['feishu-approval-subscription'] });
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
        maxDevicesControlId: clean(values.maxDevicesControlId),
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
        <Typography.Title level={4} style={{ margin: '0 0 4px' }}>集成设置</Typography.Title>
        <Typography.Text type="secondary">
          管理飞书登录、审批和外部选项。秘密仅显示是否已设置，不会回显。
        </Typography.Text>
      </div>
      {query.data?.restartRequired && (
        <Alert type="warning" showIcon message="配置已保存但尚未应用，请点击全局“重启服务端”" />
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
        disabled={mutation.isPending || subscriptionMutation.isPending || query.isLoading || query.isError || !query.data}
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

        <Card title="飞书用户状态同步" style={{ marginTop: 16 }}>
          <Typography.Paragraph>
            只同步已绑定飞书的 VPN 账号；手动创建且未绑定的账号不受影响。可在用户管理中绑定用户、查看飞书状态和手动同步。
            冻结、离职、退出或未激活会限制登录并断开 VPN；恢复后仍受管理员禁用及审批有效期约束。
          </Typography.Paragraph>
          <Typography.Paragraph>
            启用飞书登录并重启后，会自动逐项校对已绑定身份。请为应用开通通讯录用户信息、状态及邮箱读取权限，并将目标人员加入可见范围。
          </Typography.Paragraph>
          <Typography.Paragraph>
            已启用审批时，继续使用原审批事件请求地址，添加员工信息变更、员工离职和员工入职事件即可；仅使用通讯录同步时，请使用下方地址。
          </Typography.Paragraph>
          <Typography.Text code copyable>{`${window.location.origin}/api/v1/integrations/feishu/contact-events`}</Typography.Text>
          <Typography.Paragraph type="secondary">
            事件使用下方同一组 Verification Token 和 Encrypt Key。审批的“订阅审批事件”按钮不会代替通讯录事件订阅。
          </Typography.Paragraph>
        </Card>

        <Card title="飞书审批" style={{ marginTop: 16 }}>
          <Form.Item name="approvalEnabled" label="启用飞书审批" valuePropName="checked"><Switch /></Form.Item>
          <Form.Item name="approvalCode" label="审批 Code"><Input /></Form.Item>
          <Form.Item name="groupControlId" label="用户组控件 ID"><Input /></Form.Item>
          <Form.Item name="expiryControlId" label="到期日控件 ID"><Input /></Form.Item>
          <Form.Item name="reasonControlId" label="申请原因控件 ID"><Input /></Form.Item>
          <Form.Item name="maxDevicesControlId" label="终端上限控件 ID" extra="可选；配置后审批表单须填写 1–100 的整数，审批通过后更新终端上限。留空则保持原有上限。"><Input /></Form.Item>
          <SecretField formName="verificationToken" clearName="clearVerificationToken" label="Verification Token" isSet={desired?.feishuApproval.verificationTokenSet} />
          <SecretField formName="encryptKey" clearName="clearEncryptKey" label="Encrypt Key" isSet={desired?.feishuApproval.encryptKeySet} />
          <Space direction="vertical" style={{ width: '100%' }}>
            <Typography.Text strong>审批事件订阅</Typography.Text>
            <Typography.Text type="secondary">
              请先保存配置并重启服务端，再为当前生效的应用和审批 Code 执行订阅。同一应用和审批 Code 通常只需成功执行一次。
            </Typography.Text>
            {subscriptionQuery.isError ? (
              <Alert type="error" showIcon message="加载订阅执行记录失败" description={(subscriptionQuery.error as Error).message} />
            ) : subscriptionQuery.data ? (
              <>
                <Typography.Text>
                  当前生效：App ID {subscriptionQuery.data.appId || '未配置'}；审批 Code {subscriptionQuery.data.approvalCode || '未配置'}
                </Typography.Text>
                <Typography.Text>
                  {subscriptionQuery.data.lastSuccessAt != null
                    ? `本系统上次订阅成功：${new Date(subscriptionQuery.data.lastSuccessAt).toLocaleString()}`
                    : '本系统尚无此应用和审批 Code 的成功订阅记录'}
                </Typography.Text>
              </>
            ) : (
              <Typography.Text type="secondary">正在加载订阅执行记录…</Typography.Text>
            )}
            <Typography.Text type="secondary">
              此处显示本系统记录的成功执行时间，不代表飞书实时订阅状态；无记录也可能已通过其他方式订阅。记录会在服务重启后保留。
            </Typography.Text>
            <Space>
              <Button
                loading={subscriptionMutation.isPending}
                disabled={dirty || mutation.isPending || query.isError || !query.data || query.data.restartRequired || subscriptionQuery.isFetching || subscriptionQuery.isError || !subscriptionQuery.data?.canSubscribe}
                onClick={() => subscriptionMutation.mutate()}
              >{subscriptionQuery.data?.lastSuccessAt != null ? '重新执行订阅' : '订阅审批事件'}</Button>
              <Button
                loading={subscriptionQuery.isFetching}
                disabled={mutation.isPending || subscriptionMutation.isPending || !query.data}
                onClick={() => { void subscriptionQuery.refetch(); }}
              >刷新记录</Button>
            </Space>
            {(dirty || query.data?.restartRequired || (subscriptionQuery.data && !subscriptionQuery.data.canSubscribe)) && (
              <Typography.Text type="warning">
                {dirty ? '有未保存的修改，请先保存配置。' : query.data?.restartRequired ? '配置尚未生效，请先重启服务端。' : '请先启用并完整配置飞书审批，保存后重启服务端。'}
              </Typography.Text>
            )}
          </Space>
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
            disabled={subscriptionMutation.isPending || query.isLoading || query.isError || !query.data}
          >保存</Button>
          <Button
            disabled={subscriptionMutation.isPending || !dirty}
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
