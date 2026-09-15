import defaultApprovalHtml from '../../../templates/approval-approved.html?raw';
import { useEffect } from 'react';
import {
  Alert,
  App,
  Button,
  Card,
  Col,
  Form,
  Input,
  InputNumber,
  Row,
  Select,
  Space,
  Switch,
  Table,
  Tag,
  Typography,
  type TableColumnsType,
} from 'antd';
import {
  BellOutlined,
  LinkOutlined,
  MailOutlined,
  SaveOutlined,
  SendOutlined,
} from '@ant-design/icons';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import dayjs from 'dayjs';

import { systemApi } from '@/services/auth';
import type {
  EmailNotificationSettings,
  UpdateEmailNotificationSettingsRequest,
  NotificationEventView,
} from '@/types/api';

const { Title, Text, Paragraph } = Typography;

interface NotificationFormValues {
  approvalEmailTemplate: { subject: string; body: string; html?: boolean };
  enabled: boolean;
  smtpHost?: string;
  smtpPort: number;
  smtpUsername?: string;
  smtpPassword?: string;
  from?: string;
  recipients: string[];
  quietMinutes: number;
  gatewayOfflineEnabled: boolean;
  gatewayRecoveredEnabled: boolean;
  webhook: ChannelFormValues;
  feishu: ChannelFormValues;
  dingtalk: ChannelFormValues;
}

interface ChannelFormValues {
  enabled: boolean;
  url?: string;
}

function toFormValues(settings: EmailNotificationSettings): NotificationFormValues {
  return {
    approvalEmailTemplate: settings.approvalEmailTemplate,
    enabled: settings.enabled,
    smtpHost: settings.smtpHost ?? undefined,
    smtpPort: settings.smtpPort || 587,
    smtpUsername: settings.smtpUsername ?? undefined,
    smtpPassword: undefined,
    from: settings.from ?? undefined,
    recipients: settings.recipients ?? [],
    quietMinutes: settings.quietMinutes ?? 30,
    gatewayOfflineEnabled: settings.gatewayOfflineEnabled ?? true,
    gatewayRecoveredEnabled: settings.gatewayRecoveredEnabled ?? true,
    webhook: toChannelValues(settings.webhook),
    feishu: toChannelValues(settings.feishu),
    dingtalk: toChannelValues(settings.dingtalk),
  };
}

function toChannelValues(channel?: { enabled: boolean; url: string | null }): ChannelFormValues {
  return {
    enabled: channel?.enabled ?? false,
    url: channel?.url ?? undefined,
  };
}

function clean(value?: string): string | null {
  const trimmed = value?.trim();
  return trimmed ? trimmed : null;
}

export function NotificationSettingsPage() {
  const { message } = App.useApp();
  const queryClient = useQueryClient();
  const [form] = Form.useForm<NotificationFormValues>();
  const enabled = Form.useWatch('enabled', form);
  const approvalTemplate = Form.useWatch('approvalEmailTemplate', form);
  const previewValues: Record<string, string> = {
    username: 'zhangsan', applicant_email: 'zhangsan@example.com',
    user_groups: '研发组、运维组', expires_at: '2026-12-01 00:00:00（北京时间）',
    instance_code: 'example-approval-instance',
  };
  const previewTemplate = (text?: string) => (text ?? '').replace(
    /\{\{\s*([a-z_]+)\s*\}\}/g, (match, name: string) => previewValues[name] ?? match
  );

  const { data, isLoading } = useQuery({
    queryKey: ['email-notification-settings'],
    queryFn: () => systemApi.getEmailNotificationSettings(),
  });
  const { data: events, isLoading: eventsLoading } = useQuery({
    queryKey: ['notification-events'],
    queryFn: () => systemApi.listNotificationEvents({ limit: 50 }),
    refetchInterval: 15_000,
  });

  useEffect(() => {
    if (data) {
      form.setFieldsValue(toFormValues(data));
    }
  }, [data, form]);

  const updateMutation = useMutation({
    mutationFn: systemApi.updateEmailNotificationSettings,
    onSuccess: (settings) => {
      message.success('通知配置已保存');
      queryClient.setQueryData(['email-notification-settings'], settings);
      form.setFieldsValue(toFormValues(settings));
      queryClient.invalidateQueries({ queryKey: ['notification-events'] });
    },
    onError: (err) => {
      message.error(err instanceof Error ? err.message : '保存通知配置失败');
    },
  });
  const testMutation = useMutation({
    mutationFn: systemApi.sendTestEmail,
    onSuccess: () => {
      message.success('测试邮件已发送');
      queryClient.invalidateQueries({ queryKey: ['notification-events'] });
    },
    onError: (err) => {
      message.error(err instanceof Error ? err.message : '测试邮件发送失败');
      queryClient.invalidateQueries({ queryKey: ['notification-events'] });
    },
  });

  const submit = async () => {
    const values = await form.validateFields();
    const payload: UpdateEmailNotificationSettingsRequest = {
      approvalEmailTemplate: values.approvalEmailTemplate,
      enabled: values.enabled,
      smtpHost: clean(values.smtpHost),
      smtpPort: values.smtpPort,
      smtpUsername: clean(values.smtpUsername),
      from: clean(values.from),
      recipients: (values.recipients ?? []).map((s) => s.trim()).filter(Boolean),
      quietMinutes: values.quietMinutes,
      gatewayOfflineEnabled: values.gatewayOfflineEnabled,
      gatewayRecoveredEnabled: values.gatewayRecoveredEnabled,
      webhook: { enabled: values.webhook?.enabled ?? false, url: clean(values.webhook?.url) },
      feishu: { enabled: values.feishu?.enabled ?? false, url: clean(values.feishu?.url) },
      dingtalk: { enabled: values.dingtalk?.enabled ?? false, url: clean(values.dingtalk?.url) },
    };
    const password = values.smtpPassword?.trim();
    if (password !== undefined && password.length > 0) {
      payload.smtpPassword = password;
    }
    updateMutation.mutate(payload);
  };

  const sendTest = async () => {
    const values = await form.validateFields();
    const recipient = (values.recipients ?? [])[0];
    testMutation.mutate({ recipient });
  };

  const columns: TableColumnsType<NotificationEventView> = [
    {
      title: '时间',
      dataIndex: 'createdAt',
      width: 160,
      render: (value: number) => dayjs(value).format('MM-DD HH:mm:ss'),
    },
    {
      title: '事件',
      dataIndex: 'eventType',
      width: 130,
      render: (value: string) => eventLabel(value),
    },
    {
      title: '渠道',
      dataIndex: 'channel',
      width: 90,
      render: (value: string) => <Tag>{channelLabel(value)}</Tag>,
    },
    {
      title: '状态',
      dataIndex: 'status',
      width: 100,
      render: (value: string) => <Tag color={statusColor(value)}>{statusLabel(value)}</Tag>,
    },
    {
      title: '目标',
      dataIndex: 'target',
      width: 200,
      ellipsis: true,
    },
    {
      title: '主题 / 错误',
      dataIndex: 'subject',
      render: (_, record) => (
        <Space direction="vertical" size={2}>
          <Text>{record.subject}</Text>
          {record.error ? <Text type="danger">{record.error}</Text> : null}
        </Space>
      ),
    },
  ];

  return (
    <div>
      <div className="page-heading">
        <div>
          <span className="bp-eyebrow">事件通知</span>
          <Title level={4} style={{ margin: '6px 0 0' }}>
            邮件通知设置
          </Title>
        </div>
        <Button
          type="primary"
          icon={<SaveOutlined />}
          loading={updateMutation.isPending}
          onClick={submit}
        >
          保存配置
        </Button>
      </div>

      <Space direction="vertical" size={16} style={{ width: '100%' }}>
        <Alert
          showIcon
          type="info"
          message="当前支持：审批通过邮件、站点网关离线、站点网关恢复、测试邮件"
          description="审批通过邮件发送给申请人的飞书企业邮箱，不使用下方管理员收件人。普通节点离线不会发送邮件；静默期内同一网关同一事件不会重复发送，历史里会记录 skipped。"
        />

        <Card loading={isLoading}>
          <Form
            form={form}
            layout="vertical"
            initialValues={{
              enabled: false,
              smtpPort: 587,
              recipients: [],
              quietMinutes: 30,
              gatewayOfflineEnabled: true,
              gatewayRecoveredEnabled: true,
              webhook: { enabled: false },
              feishu: { enabled: false },
              dingtalk: { enabled: false },
            }}
          >
            <div className="settings-section-heading">
              <Space>
                <BellOutlined />
                <Text strong>通知开关</Text>
              </Space>
              <Form.Item name="enabled" valuePropName="checked" noStyle>
                <Switch checkedChildren="启用" unCheckedChildren="停用" />
              </Form.Item>
            </div>

            <Row gutter={16}>
              <Col xs={24} md={8}>
                <Form.Item
                  name="quietMinutes"
                  label="静默期"
                  tooltip="同一网关同一事件在静默期内只通知一次，0 表示不去重。"
                  rules={[{ required: true, message: '请输入静默期' }]}
                >
                  <InputNumber min={0} max={1440} addonAfter="分钟" style={{ width: '100%' }} />
                </Form.Item>
              </Col>
              <Col xs={24} md={8}>
                <Form.Item name="gatewayOfflineEnabled" label="网关离线" valuePropName="checked">
                  <Switch checkedChildren="通知" unCheckedChildren="忽略" />
                </Form.Item>
              </Col>
              <Col xs={24} md={8}>
                <Form.Item name="gatewayRecoveredEnabled" label="网关恢复" valuePropName="checked">
                  <Switch checkedChildren="通知" unCheckedChildren="忽略" />
                </Form.Item>
              </Col>
            </Row>

            <div className="settings-section-heading notification-channel-heading">
              <Space>
                <MailOutlined />
                <Text strong>邮件渠道</Text>
                <Tag color="blue">email</Tag>
              </Space>
              <Button
                icon={<SendOutlined />}
                loading={testMutation.isPending}
                disabled={!enabled}
                onClick={sendTest}
              >
                发送测试通知
              </Button>
            </div>

            <Row gutter={16}>
              <Col xs={24} md={16}>
                <Form.Item
                  name="smtpHost"
                  label="SMTP 服务器"
                  rules={[{ required: enabled, message: '请输入 SMTP 服务器' }]}
                >
                  <Input prefix={<MailOutlined />} placeholder="smtp.example.com" />
                </Form.Item>
              </Col>
              <Col xs={24} md={8}>
                <Form.Item
                  name="smtpPort"
                  label="端口"
                  rules={[{ required: enabled, message: '请输入端口' }]}
                >
                  <InputNumber min={1} max={65535} style={{ width: '100%' }} />
                </Form.Item>
              </Col>
            </Row>

            <Row gutter={16}>
              <Col xs={24} md={12}>
                <Form.Item name="smtpUsername" label="SMTP 用户名">
                  <Input placeholder="notice@example.com" autoComplete="off" />
                </Form.Item>
              </Col>
              <Col xs={24} md={12}>
                <Form.Item name="smtpPassword" label="SMTP 密码">
                  <Input.Password
                    placeholder={data?.smtpPasswordSet ? '已设置，留空不修改' : '请输入 SMTP 密码'}
                    autoComplete="new-password"
                  />
                </Form.Item>
              </Col>
            </Row>

            <Form.Item
              name="from"
              label="发件人"
              rules={[
                { required: enabled, message: '请输入发件人' },
                { type: 'email', message: '请输入有效邮箱地址' },
              ]}
            >
              <Input placeholder="notice@example.com" />
            </Form.Item>

            <Form.Item
              name="recipients"
              label="收件人"
              rules={[
                {
                  validator: (_, value?: string[]) => {
                    if (!enabled || (value && value.length > 0)) return Promise.resolve();
                    return Promise.reject(new Error('请至少添加一个收件人'));
                  },
                },
              ]}
            >
              <SelectRecipients />
            </Form.Item>

            <Paragraph type="secondary" style={{ marginBottom: 0 }}>
              配置保存后立即生效；无需重启服务端。
              {data?.smtpPasswordSet ? <Tag style={{ marginLeft: 8 }}>密码已保存</Tag> : null}
            </Paragraph>

            <div className="settings-section-heading"><Text strong>审批通过邮件模板</Text></div>
            <Paragraph type="secondary">
              发送至申请人的飞书企业邮箱。修改后保存，待发送及重试邮件使用最新模板。支持 HTML 样式与纯文本。HTML 建议使用行内样式，变量放在文本位置。
            </Paragraph>
            <Paragraph>
              {'可用变量：{{username}} 账号、{{applicant_email}} 申请人邮箱、{{user_groups}} 用户组、{{expires_at}} 到期时间（北京时间）、{{instance_code}} 审批实例编号。'}
            </Paragraph>
            <Space wrap style={{ marginBottom: 16 }}>
              <Form.Item name={['approvalEmailTemplate', 'html']} valuePropName="checked" label="HTML 邮件" style={{ marginBottom: 0 }}>
                <Switch />
              </Form.Item>
              <Button onClick={() => form.setFieldValue('approvalEmailTemplate', {
                subject: '易链通知：VPN 审批已通过', body: defaultApprovalHtml, html: true,
              })}>使用默认 HTML 模板</Button>
            </Space>
            <Form.Item name={['approvalEmailTemplate', 'subject']} label="审批邮件主题"
              rules={[{ required: true, message: '请输入邮件主题' }, { max: 200 }]}>
              <Input maxLength={200} />
            </Form.Item>
            <Form.Item name={['approvalEmailTemplate', 'body']} label="审批邮件正文"
              rules={[{ required: true, message: '请输入邮件正文' }, { max: 20000 }]}>
              <Input.TextArea rows={12} showCount maxLength={20000} style={{ fontFamily: 'monospace', fontSize: 12 }} />
            </Form.Item>

            <details style={{ marginBottom: 24 }}>
              <summary>模板预览（示例数据，不会发送邮件）</summary>
              <Paragraph strong>{previewTemplate(approvalTemplate?.subject)}</Paragraph>
              {approvalTemplate?.html ? (
                <iframe title="审批邮件 HTML 预览" sandbox="" referrerPolicy="no-referrer"
                  srcDoc={'<meta http-equiv="Content-Security-Policy" content="default-src \'none\'; style-src \'unsafe-inline\'; img-src data:;">' + previewTemplate(approvalTemplate?.body)}
                  style={{ width: '100%', height: 780, border: '1px solid #e2e8f0', borderRadius: 12, background: '#f1f5f9' }} />
              ) : <Paragraph style={{ whiteSpace: 'pre-wrap' }}>{previewTemplate(approvalTemplate?.body)}</Paragraph>}
            </details>

            <div className="settings-section-heading notification-channel-heading">
              <Space>
                <LinkOutlined />
                <Text strong>HTTP 渠道</Text>
                <Tag>Webhook</Tag>
                <Tag>飞书</Tag>
                <Tag>钉钉</Tag>
              </Space>
            </div>

            <ChannelFields
              name="webhook"
              title="通用 Webhook"
              placeholder="https://example.com/yilian/events"
            />
            <ChannelFields
              name="feishu"
              title="飞书机器人"
              placeholder="https://open.feishu.cn/open-apis/bot/v2/hook/..."
            />
            <ChannelFields
              name="dingtalk"
              title="钉钉机器人"
              placeholder="https://oapi.dingtalk.com/robot/send?access_token=..."
            />
          </Form>
        </Card>

        <Card
          title="通知历史"
          extra={
            <Button onClick={() => queryClient.invalidateQueries({ queryKey: ['notification-events'] })}>
              刷新
            </Button>
          }
        >
          <Table<NotificationEventView>
            rowKey="id"
            loading={eventsLoading}
            columns={columns}
            dataSource={events ?? []}
            pagination={false}
            scroll={{ x: 860 }}
          />
        </Card>
      </Space>
    </div>
  );
}

function eventLabel(value: string): string {
  if (value === 'gateway_offline') return '网关离线';
  if (value === 'gateway_recovered') return '网关恢复';
  if (value === 'approval_approved') return '审批通过';
  if (value === 'test_email') return '测试邮件';
  return value;
}

function channelLabel(value: string): string {
  if (value === 'email') return '邮件';
  if (value === 'webhook') return 'Webhook';
  if (value === 'feishu') return '飞书';
  if (value === 'dingtalk') return '钉钉';
  return value;
}

function statusLabel(value: string): string {
  if (value === 'sent') return '已发送';
  if (value === 'failed') return '失败';
  if (value === 'skipped') return '已跳过';
  return value;
}

function statusColor(value: string): string {
  if (value === 'sent') return 'success';
  if (value === 'failed') return 'error';
  if (value === 'skipped') return 'default';
  return 'processing';
}

function SelectRecipients({
  value,
  onChange,
}: {
  value?: string[];
  onChange?: (value: string[]) => void;
}) {
  return (
    <Select
      mode="tags"
      value={value}
      onChange={onChange}
      tokenSeparators={[',', '，', ' ']}
      placeholder="输入邮箱后回车，例如 ops@example.com"
    />
  );
}

function ChannelFields({
  name,
  title,
  placeholder,
}: {
  name: 'webhook' | 'feishu' | 'dingtalk';
  title: string;
  placeholder: string;
}) {
  const enabled = Form.useWatch([name, 'enabled']);
  return (
    <Row gutter={16}>
      <Col xs={24} md={6}>
        <Form.Item name={[name, 'enabled']} label={title} valuePropName="checked">
          <Switch checkedChildren="启用" unCheckedChildren="停用" />
        </Form.Item>
      </Col>
      <Col xs={24} md={18}>
        <Form.Item
          name={[name, 'url']}
          label="URL"
          rules={[
            { required: enabled, message: `请输入${title} URL` },
            { type: 'url', message: '请输入有效 URL' },
          ]}
        >
          <Input placeholder={placeholder} />
        </Form.Item>
      </Col>
    </Row>
  );
}
