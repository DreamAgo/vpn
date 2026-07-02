/**
 * 编辑「服务端 LAN 网段（VPN_SERVER_ROUTES）」弹窗。
 *
 * - 服务端作为网关下发给客户端的网段（如所在 Docker / 物理 LAN）。
 * - Select mode="tags" 自由输入多个 IPv4 CIDR，前端即时校验。
 * - 提交 PUT /admin/system/routes；保存后对新接入/重连的客户端生效。
 */
import { useEffect } from 'react';
import { Modal, Form, App, Alert } from 'antd';
import { useMutation } from '@tanstack/react-query';

import { systemApi } from '@/services/auth';
import { ApiError } from '@/services/http';
import { ErrorCodes } from '@/types/api';
import { CidrRoutesSelect } from './CidrRoutesSelect';
import { isValidCidr } from '@/utils/cidr';

interface Props {
  open: boolean;
  onClose: () => void;
  current: string[];
  onSaved: () => void;
}

interface FormValues {
  routes: string[];
}

export function EditServerRoutesModal({ open, onClose, current, onSaved }: Props) {
  const { message } = App.useApp();
  const [form] = Form.useForm<FormValues>();
  const watched = Form.useWatch('routes', form);
  const submitDisabled = (watched ?? []).some((s) => !isValidCidr(s));

  // 打开时把最新的 current 灌进表单。该 Modal 常驻挂载，仅靠 initialValues 时 current 变化
  // 后不会刷新，保存后再打开会显示陈旧值（与 EditGroupModal/SubnetsPage 的 reset-on-open 一致）。
  useEffect(() => {
    if (open) {
      form.setFieldsValue({ routes: current ?? [] });
    }
  }, [open, current, form]);

  const mutation = useMutation({
    mutationFn: (values: FormValues) =>
      systemApi.updateServerRoutes(
        values.routes.map((s) => s.trim()).filter((s) => s.length > 0)
      ),
    onSuccess: () => {
      message.success('服务端 LAN 网段已保存（对新接入/重连的客户端生效）');
      onSaved();
      onClose();
    },
    onError: (err) => {
      if (err instanceof ApiError) {
        if (err.code === ErrorCodes.NoAccess || err.code === ErrorCodes.RequireAdmin) {
          message.error('无权限执行该操作');
          return;
        }
        message.error(err.message || '保存失败');
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
      title="编辑服务端 LAN 网段"
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
        message="服务端 LAN 网段"
        description="服务端作为网关时背后的网段（如所在 Docker / 物理内网）。保存后会下发给客户端的 allowed_routes，新接入或重连的客户端即可经 VPN 访问这些网段。"
      />
      <Form<FormValues>
        form={form}
        layout="vertical"
        autoComplete="off"
        preserve={false}
        onFinish={(values) => mutation.mutate(values)}
      >
        <CidrRoutesSelect
          name="routes"
          label="LAN 网段（CIDR）"
          extra="格式示例：172.31.100.0/24、10.0.0.0/8。可添加多个，留空表示清空。"
        />
      </Form>
    </Modal>
  );
}
