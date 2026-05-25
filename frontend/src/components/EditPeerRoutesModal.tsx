/**
 * 编辑节点「站点 LAN 网段（routed_subnets）」弹窗。
 *
 * - 表单用 Select mode="tags" 自由输入多个 IPv4 CIDR。
 * - 前端做基本 CIDR 校验（a.b.c.d/n，n 0-32）即时反馈。
 * - 提交 PATCH /admin/peers/:id；后端校验失败（如 ConfigError 5004）用 message.error 展示 ApiError.message。
 */
import { Modal, Form, Select, App, Typography, Alert } from 'antd';
import { useMutation } from '@tanstack/react-query';

import { peersApi } from '@/services/peers';
import { ApiError } from '@/services/http';
import { ErrorCodes } from '@/types/api';
import type { AdminPeerView } from '@/types/api';

const { Text } = Typography;

interface Props {
  open: boolean;
  onClose: () => void;
  peer: AdminPeerView;
  onSaved: () => void;
}

interface FormValues {
  routedSubnets: string[];
}

/** 校验单个 IPv4 CIDR：a.b.c.d/n（0-255 每段、n 为 0-32）。 */
function isValidCidr(cidr: string): boolean {
  const match = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})\/(\d{1,2})$/.exec(cidr.trim());
  if (!match) return false;
  const octets = [match[1], match[2], match[3], match[4]].map((s) => Number(s));
  if (octets.some((n) => n < 0 || n > 255)) return false;
  const prefix = Number(match[5]);
  return prefix >= 0 && prefix <= 32;
}

export function EditPeerRoutesModal({ open, onClose, peer, onSaved }: Props) {
  const { message } = App.useApp();
  const [form] = Form.useForm<FormValues>();
  const watched = Form.useWatch('routedSubnets', form);
  const submitDisabled = (watched ?? []).some((s) => !isValidCidr(s));

  const mutation = useMutation({
    mutationFn: (values: FormValues) =>
      peersApi.updatePeerRoutes(
        peer.id,
        values.routedSubnets.map((s) => s.trim()).filter((s) => s.length > 0)
      ),
    onSuccess: () => {
      message.success('路由网段已保存');
      onSaved();
      onClose();
    },
    onError: (err) => {
      if (err instanceof ApiError) {
        if (err.code === ErrorCodes.NoAccess || err.code === ErrorCodes.RequireAdmin) {
          message.error('无权限执行该操作');
          return;
        }
        if (err.code === ErrorCodes.PeerNotFound) {
          message.error('节点不存在或已被移除');
          return;
        }
        message.error(err.message || '保存路由网段失败');
        return;
      }
      message.error('网络异常，请稍后再试');
    },
  });

  const handleCancel = () => {
    if (mutation.isPending) return;
    onClose();
  };

  return (
    <Modal
      open={open}
      onCancel={handleCancel}
      title="编辑路由网段"
      maskClosable={false}
      confirmLoading={mutation.isPending}
      okText="保存"
      cancelText="取消"
      okButtonProps={{ disabled: submitDisabled }}
      onOk={() => form.submit()}
    >
      <Alert
        type="info"
        showIcon
        style={{ marginBottom: 16 }}
        message="站点 LAN 网段"
        description="该节点作为站点网关时背后的内网网段，保存后其他节点可经 VPN 访问这些网段。"
      />
      <Form<FormValues>
        form={form}
        layout="vertical"
        autoComplete="off"
        preserve={false}
        initialValues={{ routedSubnets: peer.routedSubnets ?? [] }}
        onFinish={(values) => mutation.mutate(values)}
      >
        <Form.Item
          name="routedSubnets"
          label="LAN 网段（CIDR）"
          rules={[
            {
              validator: (_rule, value: string[] | undefined) => {
                const invalid = (value ?? []).filter((s) => !isValidCidr(s));
                if (invalid.length > 0) {
                  return Promise.reject(
                    new Error(`存在非法 CIDR：${invalid.join('、')}`)
                  );
                }
                return Promise.resolve();
              },
            },
          ]}
        >
          <Select
            mode="tags"
            allowClear
            placeholder="输入 IPv4 CIDR，如 192.168.10.0/24，回车添加"
            tokenSeparators={[',', ' ', '\n']}
            open={false}
            style={{ width: '100%' }}
          />
        </Form.Item>
        <Text type="secondary">格式示例：192.168.10.0/24、10.0.0.0/8。可添加多个，留空表示无路由网段。</Text>
      </Form>
    </Modal>
  );
}
