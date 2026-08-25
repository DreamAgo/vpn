import { useEffect, useRef } from 'react';
import { Alert, App, Button, Card, Col, Form, InputNumber, Radio, Row, Space, Typography } from 'antd';
import { SaveOutlined, SwapOutlined } from '@ant-design/icons';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';

import { systemApi } from '@/services/auth';
import type { NetworkSettings } from '@/types/api';

const { Title, Paragraph, Text } = Typography;
const NETWORK_SETTINGS_QUERY_KEY = ['network-settings'];

export function NetworkSettingsPage() {
  const { message } = App.useApp();
  const queryClient = useQueryClient();
  const [form] = Form.useForm<NetworkSettings>();
  const mode = Form.useWatch('mode', form);
  const editVersionRef = useRef(0);
  const appliedVersionRef = useRef(0);
  const hydratedRef = useRef(false);

  const { data, error, isError, isLoading, isFetching, refetch } = useQuery({
    queryKey: NETWORK_SETTINGS_QUERY_KEY,
    queryFn: () => systemApi.getNetworkSettings(),
  });

  useEffect(() => {
    if (
      data &&
      (!hydratedRef.current || editVersionRef.current === appliedVersionRef.current)
    ) {
      form.setFieldsValue(data);
      hydratedRef.current = true;
      appliedVersionRef.current = editVersionRef.current;
    }
  }, [data, form]);

  const updateMutation = useMutation({
    mutationFn: async ({ settings, editVersion }: { settings: NetworkSettings; editVersion: number }) => ({
      settings: await systemApi.updateNetworkSettings(settings),
      editVersion,
    }),
    onSuccess: ({ settings, editVersion }) => {
      queryClient.setQueryData(NETWORK_SETTINGS_QUERY_KEY, settings);
      if (editVersionRef.current === editVersion) {
        form.setFieldsValue(settings);
        appliedVersionRef.current = editVersion;
      }
      message.success('网络参数已保存');
    },
    onError: (error) => {
      message.error(error instanceof Error ? error.message : '保存网络参数失败');
    },
  });

  const submit = async () => {
    const values = await form.validateFields();
    updateMutation.mutate({ settings: values, editVersion: editVersionRef.current });
  };

  return (
    <div>
      <div className="page-heading">
        <div>
          <span className="bp-eyebrow">数据面策略</span>
          <Title level={4} style={{ margin: '6px 0 0' }}>网络设置</Title>
        </div>
        <Button
          type="primary"
          icon={<SaveOutlined />}
          disabled={!data || isError}
          loading={updateMutation.isPending || (isFetching && !data)}
          onClick={submit}
        >
          保存配置
        </Button>
      </div>

      <Space direction="vertical" size={16} style={{ width: '100%' }}>
        {isError ? (
          <Alert
            showIcon
            type="error"
            message="网络参数加载失败"
            description={error instanceof Error ? error.message : '请检查网络连接后重试。'}
            action={<Button onClick={() => void refetch()}>重新加载</Button>}
          />
        ) : null}
        <Alert
          showIcon
          type="info"
          message="保存后仅对新连接或重连生效"
          description="在线节点不会被强制断开。环境变量只在数据库没有整组网络配置时用于首次初始化，之后以此页面保存的值为准。"
        />
        <Card loading={isLoading}>
          <Form
            form={form}
            layout="vertical"
            disabled={!data || isError || updateMutation.isPending}
            onValuesChange={() => {
              editVersionRef.current += 1;
            }}
            initialValues={{ mode: 'fixed', defaultMtu: 1360, minMtu: 1280, maxMtu: 1420 }}
          >
            <Form.Item name="mode" label="MTU 模式" rules={[{ required: true }]}>
              <Radio.Group optionType="button" buttonStyle="solid">
                <Radio.Button value="fixed">固定</Radio.Button>
                <Radio.Button value="auto">自动</Radio.Button>
              </Radio.Group>
            </Form.Item>
            <Paragraph type="secondary">
              {mode === 'auto'
                ? '自动模式按当前传输路径计算 MTU，并限制在最小值和最大值之间；无混淆传输时使用默认值。'
                : '固定模式在所有平台直接使用默认 MTU。'}
            </Paragraph>

            <Row gutter={16}>
              <Col xs={24} md={8}>
                <Form.Item
                  name="minMtu"
                  label="最小 MTU"
                  dependencies={['defaultMtu', 'maxMtu']}
                  rules={[
                    { required: true, message: '请输入最小 MTU' },
                    ({ getFieldValue }) => ({
                      validator: (_, value) =>
                        value >= 1280 && value <= getFieldValue('defaultMtu')
                          ? Promise.resolve()
                          : Promise.reject(new Error('必须在 1280 与默认 MTU 之间')),
                    }),
                  ]}
                >
                  <InputNumber min={1280} max={1420} precision={0} style={{ width: '100%' }} />
                </Form.Item>
              </Col>
              <Col xs={24} md={8}>
                <Form.Item
                  name="defaultMtu"
                  label="默认 MTU"
                  dependencies={['minMtu', 'maxMtu']}
                  rules={[
                    { required: true, message: '请输入默认 MTU' },
                    ({ getFieldValue }) => ({
                      validator: (_, value) =>
                        value >= getFieldValue('minMtu') && value <= getFieldValue('maxMtu')
                          ? Promise.resolve()
                          : Promise.reject(new Error('必须位于最小 MTU 与最大 MTU 之间')),
                    }),
                  ]}
                >
                  <InputNumber min={1280} max={1420} precision={0} style={{ width: '100%' }} />
                </Form.Item>
              </Col>
              <Col xs={24} md={8}>
                <Form.Item
                  name="maxMtu"
                  label="最大 MTU"
                  dependencies={['minMtu', 'defaultMtu']}
                  rules={[
                    { required: true, message: '请输入最大 MTU' },
                    ({ getFieldValue }) => ({
                      validator: (_, value) =>
                        value >= getFieldValue('defaultMtu') && value <= 1420
                          ? Promise.resolve()
                          : Promise.reject(new Error('必须在默认 MTU 与 1420 之间')),
                    }),
                  ]}
                >
                  <InputNumber min={1280} max={1420} precision={0} style={{ width: '100%' }} />
                </Form.Item>
              </Col>
            </Row>
            <Space>
              <SwapOutlined />
              <Text type="secondary">约束：1280 ≤ 最小值 ≤ 默认值 ≤ 最大值 ≤ 1420</Text>
            </Space>
          </Form>
        </Card>
      </Space>
    </div>
  );
}
