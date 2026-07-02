/**
 * 「分配用户组」弹窗:设置某用户所属的**多个**组(全量覆盖),或清空。
 *
 * 组决定成员的 VPN allowed_routes(访问控制,多组取并集);改动对该用户下次接入/重连生效。
 */
import { useEffect, useState } from 'react';
import { Modal, Select, App, Typography, Alert } from 'antd';
import { useQuery } from '@tanstack/react-query';

import { groupsApi } from '@/services/groups';
import { ApiError } from '@/services/http';
import type { UserDto } from '@/types/api';

const { Text } = Typography;

interface Props {
  open: boolean;
  user: UserDto | null;
  onClose: () => void;
  onSaved: () => void;
}

export function AssignGroupModal({ open, user, onClose, onSaved }: Props) {
  const { message } = App.useApp();
  const [value, setValue] = useState<string[]>([]);
  const [saving, setSaving] = useState(false);

  const { data: groups } = useQuery({
    queryKey: ['groups'],
    queryFn: groupsApi.listGroups,
    enabled: open,
  });

  useEffect(() => {
    if (open) setValue(user?.groupIds ?? []);
  }, [open, user]);

  const handleOk = async () => {
    if (!user) return;
    setSaving(true);
    try {
      await groupsApi.setUserGroups(user.id, { groupIds: value });
      message.success('已更新用户组（对该用户下次接入/重连生效）');
      onSaved();
      onClose();
    } catch (err) {
      message.error(err instanceof ApiError ? err.message || '保存失败' : '网络异常，请稍后再试');
    } finally {
      setSaving(false);
    }
  };

  const options = (groups ?? []).map((g) => ({
    value: g.id,
    label: g.routes.length > 0 ? `${g.name}（${g.routes.join('、')}）` : `${g.name}（无额外网段）`,
  }));

  return (
    <Modal
      open={open}
      title={user ? `分配用户组 · ${user.username}` : '分配用户组'}
      onCancel={() => !saving && onClose()}
      onOk={handleOk}
      confirmLoading={saving}
      okText="保存"
      cancelText="取消"
      destroyOnClose
    >
      <Alert
        type="info"
        showIcon
        style={{ marginBottom: 16 }}
        message="组决定成员可访问的网段（可多选，取并集）"
        description="该用户经 VPN 可访问的网段 = VPN 子网 + 所选各组网段的并集（+ 站点网关）。不选任何组则回退到全局默认网段。"
      />
      <Text type="secondary">所属用户组（可多选）</Text>
      <Select
        mode="multiple"
        allowClear
        style={{ width: '100%', marginTop: 6 }}
        placeholder="选择一个或多个组，留空表示不分组"
        value={value}
        onChange={setValue}
        options={options}
        showSearch
        optionFilterProp="label"
      />
    </Modal>
  );
}
