import { useEffect, useRef } from 'react';
import { Alert, App, Button, Card, Col, Form, Input, InputNumber, Radio, Row, Select, Space, Switch, Typography } from 'antd';
import { SaveOutlined } from '@ant-design/icons';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';

import { systemApi } from '@/services/auth';
import type { DataPlaneSettings, UpdateNetworkSettingsRequest } from '@/types/api';
import { isValidCidr } from '@/utils/cidr';

const { Title, Paragraph } = Typography;
const QUERY_KEY = ['network-settings'];

function changedRestartFields(applied: DataPlaneSettings, desired: DataPlaneSettings): string[] {
  const fields: Array<[string, unknown, unknown]> = [
    ['虚拟子网', applied.vpn.vpnSubnet, desired.vpn.vpnSubnet],
    ['监听端口', applied.vpn.vpnListenPort, desired.vpn.vpnListenPort],
    ['公网 Endpoint', applied.vpn.vpnEndpoint, desired.vpn.vpnEndpoint],
    ['WireGuard 后端', applied.vpn.wgBackend, desired.vpn.wgBackend],
    ['接口名', applied.vpn.wgInterface, desired.vpn.wgInterface],
    ['UDP 混淆', JSON.stringify(applied.obfs), JSON.stringify(desired.obfs)],
  ];
  return fields.filter(([, current, next]) => current !== next).map(([label]) => label);
}

export function NetworkSettingsPage() {
  const { message } = App.useApp();
  const queryClient = useQueryClient();
  const [form] = Form.useForm<UpdateNetworkSettingsRequest>();
  const mode = Form.useWatch(['desired', 'mtu', 'mode'], form);
  const obfsEnabled = Form.useWatch(['desired', 'obfs', 'enabled'], form);
  const editVersion = useRef(0);
  const hydratedVersion = useRef(0);
  const hydrated = useRef(false);
  const { data, error, isError, isLoading, refetch } = useQuery({
    queryKey: QUERY_KEY,
    queryFn: () => systemApi.getNetworkSettings(),
  });

  useEffect(() => {
    if (data && (!hydrated.current || editVersion.current === hydratedVersion.current)) {
      form.setFieldsValue({ desired: data.desired, serverRoutes: data.serverRoutes });
      hydrated.current = true;
      hydratedVersion.current = editVersion.current;
    }
  }, [data, form]);

  const mutation = useMutation({
    mutationFn: async ({ request, version }: { request: UpdateNetworkSettingsRequest; version: number }) => ({
      result: await systemApi.updateNetworkSettings(request), version,
    }),
    onSuccess: ({ result, version }) => {
      queryClient.setQueryData(QUERY_KEY, result);
      if (editVersion.current === version) {
        form.setFieldsValue({ desired: result.desired, serverRoutes: result.serverRoutes });
        hydratedVersion.current = version;
      }
      message.success(result.restartRequired ? '配置已保存，请安排重启服务端' : '网络配置已保存');
    },
    onError: (reason) => message.error(reason instanceof Error ? reason.message : '保存失败'),
  });

  const save = async () => {
    const request = await form.validateFields();
    mutation.mutate({ request, version: editVersion.current });
  };
  const endpointRule = { pattern: /^[^:\s]+:\d+$/, message: '请输入 host:port' };
  const restartChanges = data ? changedRestartFields(data.applied, data.desired) : [];

  return <div>
    <div className="page-heading">
      <div><span className="bp-eyebrow">数据面策略</span><Title level={4} style={{ margin: '6px 0 0' }}>网络设置</Title></div>
      <Button type="primary" icon={<SaveOutlined />} disabled={!data || isError} loading={mutation.isPending} onClick={() => void save()}>保存配置</Button>
    </div>
    <Space direction="vertical" size={16} style={{ width: '100%' }}>
      {isError && <Alert showIcon type="error" message="网络参数加载失败" description={error instanceof Error ? error.message : '请稍后重试'} action={<Button onClick={() => void refetch()}>重试</Button>} />}
      {data?.restartRequired && <Alert showIcon type="warning" message="存在待重启配置" description={`待生效：${restartChanges.join('、')}。服务不会自动重启或断开节点；若包含虚拟子网，重启前暂停节点注册。`} />}
      <Alert showIcon type="info" message="生效方式" description="基础 VPN 与混淆配置重启后生效；LAN 路由立即热更新；MTU 对新连接或重连生效。环境变量只用于首次初始化。" />
      <Form form={form} layout="vertical" disabled={!data || isError || mutation.isPending} onValuesChange={() => { editVersion.current += 1; }}>
        <Card title="基础 VPN" loading={isLoading}>
          <Row gutter={16}>
            <Col xs={24} md={12}><Form.Item name={['desired','vpn','vpnSubnet']} label="虚拟子网" rules={[{ required: true }, { validator: (_, value) => isValidCidr(value) ? Promise.resolve() : Promise.reject(new Error('请输入合法 IPv4 CIDR')) }]}><Input placeholder="10.8.0.0/24" /></Form.Item></Col>
            <Col xs={24} md={12}><Form.Item name={['desired','vpn','vpnEndpoint']} label="公网 Endpoint" rules={[{ required: true }, endpointRule]}><Input /></Form.Item></Col>
            <Col xs={24} md={8}><Form.Item name={['desired','vpn','vpnListenPort']} label="监听端口" rules={[{ required: true }]}><InputNumber min={1} max={65535} precision={0} style={{ width: '100%' }} /></Form.Item></Col>
            <Col xs={24} md={8}><Form.Item name={['desired','vpn','wgBackend']} label="WireGuard 后端" rules={[{ required: true }]}><Select options={[{value:'noop'},{value:'kernel'},{value:'userspace'},{value:'auto'}]} /></Form.Item></Col>
            <Col xs={24} md={8}><Form.Item name={['desired','vpn','wgInterface']} label="接口名" rules={[{ required: true, max: 15 }, { pattern: /^(?=.*[A-Za-z0-9])[A-Za-z0-9_.-]+$/, message: '仅允许字母、数字、_、-、.，且至少包含一个字母或数字' }]}><Input /></Form.Item></Col>
          </Row>
          <Paragraph type="secondary">已有任何节点记录（包括已删除记录）时不能更改虚拟子网，以免地址错配。</Paragraph>
        </Card>

        <Card title="UDP 混淆" style={{ marginTop: 16 }}>
          <Form.Item name={['desired','obfs','enabled']} label="启用混淆" valuePropName="checked"><Switch disabled={!data?.pskConfigured} /></Form.Item>
          {data && !data.pskConfigured && <Alert type="warning" showIcon message="尚未配置 VPN_OBFS_PSK，无法启用混淆" style={{ marginBottom: 16 }} />}
          <Row gutter={16}>
            <Col xs={24} md={12}><Form.Item name={['desired','obfs','mode']} label="模式" rules={[{ required: true }]}><Select disabled={!obfsEnabled} options={[{value:'low-overhead-v1',label:'低开销'},{value:'paranoid-v1',label:'全填充'}]} /></Form.Item></Col>
            <Col xs={24} md={12}><Form.Item name={['desired','obfs','pathMtu']} label="路径 MTU" rules={[{ required: true }]}><InputNumber disabled={!obfsEnabled} min={576} max={9000} precision={0} style={{width:'100%'}} /></Form.Item></Col>
            <Col xs={24} md={12}><Form.Item name={['desired','obfs','bindAddr']} label="监听地址" rules={[{ required: true }]}><Input disabled={!obfsEnabled} placeholder="0.0.0.0:47358" /></Form.Item></Col>
            <Col xs={24} md={12}><Form.Item name={['desired','obfs','publicEndpoint']} label="公网 Endpoint" rules={[{ required: true }, endpointRule]}><Input disabled={!obfsEnabled} /></Form.Item></Col>
          </Row>
        </Card>

        <Card title="LAN 路由" style={{ marginTop: 16 }}>
          <Form.Item name="serverRoutes" label="CIDR 列表" rules={[{ validator: (_, routes: string[] = []) => routes.every((route) => isValidCidr(route) && route !== '0.0.0.0/0') ? Promise.resolve() : Promise.reject(new Error('请输入合法 IPv4 CIDR，且不能使用 0.0.0.0/0')) }]}><Select mode="tags" tokenSeparators={[',']} placeholder="192.168.0.0/16" /></Form.Item>
          <Paragraph type="secondary">允许使用 10.0.0.0/8 等覆盖 VPN 子网的宽泛 LAN/组路由；禁止默认路由。</Paragraph>
        </Card>

        <Card title="隧道 MTU" style={{ marginTop: 16 }}>
          <Form.Item name={['desired','mtu','mode']} label="模式" rules={[{ required: true }]}><Radio.Group optionType="button"><Radio.Button value="fixed">固定</Radio.Button><Radio.Button value="auto">自动</Radio.Button></Radio.Group></Form.Item>
          <Paragraph type="secondary">{mode === 'auto' ? '按路径探测并限制在范围内。' : '所有平台使用默认 MTU。'}</Paragraph>
          <Row gutter={16}>
            {([['minMtu','最小 MTU'],['defaultMtu','默认 MTU'],['maxMtu','最大 MTU']] as const).map(([name,label]) => <Col xs={24} md={8} key={name}><Form.Item name={['desired','mtu',name]} label={label} dependencies={[['desired','mtu','minMtu'], ['desired','mtu','defaultMtu'], ['desired','mtu','maxMtu']]} rules={[{required:true}, ({ getFieldValue }) => ({ validator: () => { const mtu = getFieldValue(['desired','mtu']); return mtu && mtu.minMtu <= mtu.defaultMtu && mtu.defaultMtu <= mtu.maxMtu ? Promise.resolve() : Promise.reject(new Error('必须满足：最小 MTU ≤ 默认 MTU ≤ 最大 MTU')); } })]}><InputNumber min={1280} max={1420} precision={0} style={{width:'100%'}} /></Form.Item></Col>)}
          </Row>
        </Card>
      </Form>
    </Space>
  </div>;
}
