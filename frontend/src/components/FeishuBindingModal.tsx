import { useState } from 'react';
import { Modal, Form, Input, Select, Button, Space, Alert, App, Descriptions, Tag } from 'antd';
import { usersApi } from '@/services/users';
import type { UserDto, FeishuBindingDto, FeishuLookupRequest } from '@/types/api';

import { feishuStatusLabels } from '@/services/feishuStatus';

export function FeishuBindingModal({ user, onClose, onSuccess }: {
  user: UserDto; onClose: () => void; onSuccess: () => void;
}) {
  const { message } = App.useApp();
  const [form] = Form.useForm<FeishuLookupRequest>();
  const [preview, setPreview] = useState<{ request: FeishuLookupRequest; user: FeishuBindingDto } | null>(null);
  const [busy, setBusy] = useState(false);
  const bound = (user.feishuBindings?.length ?? 0) > 0;
  const lookup = async () => {
    try {
      const request = await form.validateFields();
      setBusy(true); setPreview(null);
      const found = await usersApi.lookupFeishu(request);
      setPreview({ request, user: found });
    } catch (error) {
      if (error instanceof Error) message.error(error.message || '查询失败');
    } finally { setBusy(false); }
  };
  const bind = async () => {
    if (!preview) return;
    setBusy(true);
    try {
      await usersApi.bindFeishu(user.id, preview.request);
      message.success('绑定成功，已同步飞书状态'); onSuccess(); onClose();
    } catch (error) { message.error(error instanceof Error ? error.message : '绑定失败'); }
    finally { setBusy(false); }
  };
  return <Modal title={`绑定飞书用户 · ${user.username}`} open onCancel={busy ? undefined : onClose}
    maskClosable={!busy} closable={!busy} footer={[
      <Button key="cancel" disabled={busy} onClick={onClose}>取消</Button>,
      <Button key="bind" type="primary" loading={busy} disabled={!preview || bound} onClick={bind}>确认绑定</Button>,
    ]}>
    <Space direction="vertical" style={{ width: '100%' }}>
      <Alert type="info" showIcon message="绑定后将同步飞书账号状态。冻结、离职或退出会限制登录并断开 VPN；恢复时仍遵守管理员禁用和审批有效期。" />
      <Form form={form} layout="vertical" initialValues={{ idType: 'open_id' }} onValuesChange={() => setPreview(null)} disabled={busy}>
        <Form.Item name="idType" label="飞书 ID 类型" rules={[{ required: true }]}>
          <Select options={[{value:'open_id',label:'Open ID'},{value:'user_id',label:'User ID'},{value:'union_id',label:'Union ID'}]} />
        </Form.Item>
        <Form.Item name="userId" label="飞书用户 ID" rules={[{ required: true, whitespace: true, message: '请输入飞书用户 ID' }]}>
          <Input maxLength={256} placeholder="填写飞书通讯录中的用户 ID，查询后核对姓名与邮箱" />
        </Form.Item>
        <Button onClick={lookup} loading={busy}>查询飞书用户</Button>
      </Form>
      {preview && <Descriptions column={1} bordered size="small">
        <Descriptions.Item label="姓名">{preview.user.name || '未提供'}</Descriptions.Item>
        <Descriptions.Item label="邮箱">{preview.user.email || '未提供'}</Descriptions.Item>
        <Descriptions.Item label="状态"><Tag color={preview.user.blocked ? 'error' : 'success'}>{feishuStatusLabels[preview.user.status]}</Tag></Descriptions.Item>
        <Descriptions.Item label="绑定标识">{preview.user.unionId}</Descriptions.Item>
      </Descriptions>}
      {preview?.user.blocked && <Alert type="warning" showIcon message="此飞书账号当前不可用，确认绑定后该 VPN 用户也将受到访问限制。" />}
    </Space>
  </Modal>;
}
