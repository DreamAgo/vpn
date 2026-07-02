/**
 * 设置节点「站点网关（routed_subnets）」弹窗。
 *
 * - 表单用 Select mode="tags" 自由输入多个 IPv4 CIDR。
 * - 前端做基本 CIDR 校验（a.b.c.d/n，n 0-32）即时反馈。
 * - 提交 PATCH /admin/peers/:id；后端校验失败（如 ConfigError 5004）用 message.error 展示 ApiError.message。
 */
import { Modal, Form, App, Alert } from 'antd';
import { useMutation } from '@tanstack/react-query';

import { peersApi } from '@/services/peers';
import { ApiError } from '@/services/http';
import { ErrorCodes } from '@/types/api';
import type { AdminPeerView } from '@/types/api';
import { CidrRoutesSelect } from './CidrRoutesSelect';
import { isValidCidr } from '@/utils/cidr';

interface Props {
  open: boolean;
  onClose: () => void;
  peer: AdminPeerView;
  onSaved: () => void;
}

interface FormValues {
  routedSubnets: string[];
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
      message.success('站点网关设置已保存');
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
        message.error(err.message || '保存站点网关设置失败');
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
      title="设置站点网关"
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
        message="指定该节点可转发的站点网段"
        description="填写网段后，该节点会作为这些 LAN 网段的站点网关；其他节点将通过安全链路访问这些网段。网关主机仍需在本机开启 IP 转发，并按现场网络配置路由或 NAT。"
      />
      <Form<FormValues>
        form={form}
        layout="vertical"
        autoComplete="off"
        preserve={false}
        initialValues={{ routedSubnets: peer.routedSubnets ?? [] }}
        onFinish={(values) => mutation.mutate(values)}
      >
        <CidrRoutesSelect
          name="routedSubnets"
          label="LAN 网段（CIDR）"
          extra="格式示例：192.168.10.0/24、10.0.0.0/8。可添加多个；留空表示该节点不作为站点网关。"
        />
      </Form>
    </Modal>
  );
}
