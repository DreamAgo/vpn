import { useMemo, useState } from 'react';
import {
  Alert,
  App,
  Button,
  Card,
  Form,
  Input,
  Modal,
  Space,
  Table,
  Tag,
  Typography,
  type TableColumnsType,
} from 'antd';
import { KeyOutlined, PlusOutlined, StopOutlined } from '@ant-design/icons';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import dayjs from 'dayjs';

import { apiKeysApi } from '@/services/apiKeys';
import type { ApiKeyDto, CreateApiKeyResponse } from '@/types/api';

const { Title, Text, Paragraph } = Typography;

const DEFAULT_SCOPE = 'admin:*';

function formatTime(value: number | null): React.ReactNode {
  if (!value) return <Text type="secondary">从未</Text>;
  return <span title={dayjs(value).format('YYYY-MM-DD HH:mm:ss')}>{dayjs(value).fromNow()}</span>;
}

export function ApiKeysPage() {
  const { message, modal } = App.useApp();
  const queryClient = useQueryClient();
  const [createOpen, setCreateOpen] = useState(false);
  const [created, setCreated] = useState<CreateApiKeyResponse | null>(null);
  const [form] = Form.useForm<{ name: string; scopes: string }>();

  const { data, isLoading } = useQuery({
    queryKey: ['api-keys'],
    queryFn: () => apiKeysApi.listApiKeys(),
  });

  const createMutation = useMutation({
    mutationFn: apiKeysApi.createApiKey,
    onSuccess: (resp) => {
      setCreated(resp);
      setCreateOpen(false);
      form.resetFields();
      queryClient.invalidateQueries({ queryKey: ['api-keys'] });
    },
    onError: (err) => {
      message.error(err instanceof Error ? err.message : '创建 API Key 失败');
    },
  });

  const revokeMutation = useMutation({
    mutationFn: apiKeysApi.revokeApiKey,
    onSuccess: () => {
      message.success('API Key 已吊销');
      queryClient.invalidateQueries({ queryKey: ['api-keys'] });
    },
    onError: (err) => {
      message.error(err instanceof Error ? err.message : '吊销 API Key 失败');
    },
  });

  const columns = useMemo<TableColumnsType<ApiKeyDto>>(
    () => [
      {
        title: '名称',
        dataIndex: 'name',
        render: (_, record) => (
          <Space direction="vertical" size={2}>
            <Text strong>{record.name}</Text>
            <Text type="secondary" style={{ fontSize: 12 }}>
              {record.id}
            </Text>
          </Space>
        ),
      },
      {
        title: '权限范围',
        dataIndex: 'scopes',
        render: (_, record) => (
          <Space size={[4, 4]} wrap>
            {record.scopes.map((scope) => (
              <Tag key={scope} color={scope === DEFAULT_SCOPE ? 'blue' : 'default'}>
                {scope}
              </Tag>
            ))}
          </Space>
        ),
      },
      {
        title: '状态',
        dataIndex: 'status',
        width: 110,
        render: (_, record) =>
          record.status === 'active' ? <Tag color="success">启用</Tag> : <Tag>已吊销</Tag>,
      },
      {
        title: '最近使用',
        dataIndex: 'lastUsedAt',
        width: 150,
        render: (_, record) => formatTime(record.lastUsedAt),
      },
      {
        title: '创建时间',
        dataIndex: 'createdAt',
        width: 150,
        render: (_, record) => (
          <span title={dayjs(record.createdAt).format('YYYY-MM-DD HH:mm:ss')}>
            {dayjs(record.createdAt).format('YYYY-MM-DD')}
          </span>
        ),
      },
      {
        title: '操作',
        key: 'actions',
        width: 110,
        render: (_, record) => (
          <Button
            danger
            type="link"
            size="small"
            icon={<StopOutlined />}
            disabled={record.status !== 'active'}
            loading={revokeMutation.isPending && revokeMutation.variables === record.id}
            onClick={() => {
              modal.confirm({
                title: '吊销 API Key',
                content: `确定吊销「${record.name}」吗？吊销后使用该 Key 的外部系统会立即失效。`,
                okText: '吊销',
                okButtonProps: { danger: true },
                cancelText: '取消',
                onOk: () => revokeMutation.mutateAsync(record.id),
              });
            }}
          >
            吊销
          </Button>
        ),
      },
    ],
    [modal, revokeMutation]
  );

  const submitCreate = async () => {
    const values = await form.validateFields();
    const scopes = values.scopes
      .split(',')
      .map((s) => s.trim())
      .filter(Boolean);
    createMutation.mutate({ name: values.name, scopes });
  };

  return (
    <div>
      <div className="page-heading">
        <div>
          <span className="bp-eyebrow">开放接口</span>
          <Title level={4} style={{ margin: '6px 0 0' }}>
            服务账号 API Key
          </Title>
        </div>
        <Button type="primary" icon={<PlusOutlined />} onClick={() => setCreateOpen(true)}>
          创建 API Key
        </Button>
      </div>

      <Space direction="vertical" size={16} style={{ width: '100%' }}>
        <Alert
          type="info"
          showIcon
          message="用于第三方系统调用易链开放 API"
          description="API Key 创建后只显示一次。请保存在外部系统密钥管理中，不要写入前端代码或公开仓库。"
        />

        <Card>
          <Table<ApiKeyDto>
            rowKey="id"
            loading={isLoading}
            columns={columns}
            dataSource={data ?? []}
            pagination={false}
          />
        </Card>
      </Space>

      <Modal
        title="创建 API Key"
        open={createOpen}
        onCancel={() => setCreateOpen(false)}
        onOk={submitCreate}
        okText="创建"
        confirmLoading={createMutation.isPending}
      >
        <Form
          form={form}
          layout="vertical"
          initialValues={{ scopes: DEFAULT_SCOPE }}
          style={{ marginTop: 12 }}
        >
          <Form.Item
            name="name"
            label="名称"
            rules={[{ required: true, message: '请输入名称' }]}
          >
            <Input prefix={<KeyOutlined />} placeholder="例如：billing-system" />
          </Form.Item>
          <Form.Item
            name="scopes"
            label="权限范围"
            tooltip="多个 scope 用英文逗号分隔。当前后端默认按服务账号管理员处理，scope 会先存储下来。"
          >
            <Input placeholder="admin:*" />
          </Form.Item>
        </Form>
      </Modal>

      <Modal
        title="API Key 已创建"
        open={created !== null}
        onCancel={() => setCreated(null)}
        footer={[
          <Button key="done" type="primary" onClick={() => setCreated(null)}>
            我已保存
          </Button>,
        ]}
      >
        <Alert
          type="warning"
          showIcon
          message="请立即保存密钥"
          description="关闭窗口后无法再次查看明文 API Key，只能吊销后重新创建。"
          style={{ marginBottom: 16 }}
        />
        <Paragraph copyable={{ text: created?.key ?? '' }}>
          <Text code style={{ wordBreak: 'break-all', whiteSpace: 'normal' }}>
            {created?.key}
          </Text>
        </Paragraph>
      </Modal>
    </div>
  );
}
