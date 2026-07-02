/**
 * 新建 / 编辑「用户组」弹窗。
 *
 * - 组名 + 可路由网段(CIDR,Select mode="tags" 自由输入,前端即时校验)。
 * - `group` 为 null → 新建(POST /admin/groups);否则编辑(PATCH /admin/groups/:id)。
 * - 组的可路由网段决定成员的 VPN allowed_routes(访问控制),对新接入/重连客户端生效。
 */
import { useEffect } from 'react';
import { Modal, Form, Input, App, Alert } from 'antd';
import { useMutation } from '@tanstack/react-query';

import { groupsApi } from '@/services/groups';
import { ApiError } from '@/services/http';
import { ErrorCodes } from '@/types/api';
import type { UserGroupDto } from '@/types/api';
import { CidrRoutesSelect } from './CidrRoutesSelect';
import { isValidCidr } from '@/utils/cidr';

interface Props {
  open: boolean;
  onClose: () => void;
  group: UserGroupDto | null; // null = 新建
  onSaved: () => void;
}

interface FormValues {
  name: string;
  routes: string[];
}

export function EditGroupModal({ open, onClose, group, onSaved }: Props) {
  const { message } = App.useApp();
  const [form] = Form.useForm<FormValues>();
  const watched = Form.useWatch('routes', form);
  const isEdit = group !== null;
  const submitDisabled = (watched ?? []).some((s) => !isValidCidr(s));

  // 打开时把当前组数据灌进表单。
  useEffect(() => {
    if (open) {
      form.setFieldsValue({ name: group?.name ?? '', routes: group?.routes ?? [] });
    }
  }, [open, group, form]);

  const mutation = useMutation({
    mutationFn: (values: FormValues) => {
      const routes = values.routes.map((s) => s.trim()).filter((s) => s.length > 0);
      const name = values.name.trim();
      return isEdit
        ? groupsApi.updateGroup(group!.id, { name, routes })
        : groupsApi.createGroup({ name, routes });
    },
    onSuccess: () => {
      message.success(isEdit ? '用户组已更新（对新接入/重连成员生效）' : '用户组已创建');
      onSaved();
      onClose();
    },
    onError: (err) => {
      if (err instanceof ApiError) {
        if (err.code === ErrorCodes.DuplicateResource) {
          message.error('用户组名称已存在');
          return;
        }
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
      title={isEdit ? '编辑用户组' : '新建用户组'}
      maskClosable={false}
      confirmLoading={mutation.isPending}
      okText="保存"
      cancelText="取消"
      okButtonProps={{ disabled: submitDisabled }}
      onOk={() => form.submit()}
      destroyOnClose
    >
      <Alert
        type="info"
        showIcon
        style={{ marginBottom: 16 }}
        message="组级可路由网段（访问控制）"
        description="该组成员经 VPN 可访问的网段。成员的 allowed_routes = VPN 子网 + 本组网段（含落在本组网段内的站点网关 LAN）；不含全局默认网段，也不含未被本组网段覆盖的其他站点 LAN。留空表示成员只放行 VPN 子网。"
      />
      <Form<FormValues>
        form={form}
        layout="vertical"
        autoComplete="off"
        preserve={false}
        onFinish={(values) => mutation.mutate(values)}
      >
        <Form.Item
          name="name"
          label="组名"
          rules={[
            { required: true, message: '请输入组名' },
            { whitespace: true, message: '组名不能为空白' },
          ]}
        >
          <Input placeholder="如：研发组 / 运维组" maxLength={64} />
        </Form.Item>
        <CidrRoutesSelect
          name="routes"
          label="可路由网段（CIDR）"
          extra="格式示例：172.31.100.0/24、10.0.0.0/8。回车添加多个，留空表示不放行额外网段。"
        />
      </Form>
    </Modal>
  );
}
