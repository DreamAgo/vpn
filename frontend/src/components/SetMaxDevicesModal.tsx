/**
 * 「设置终端上限」弹窗:调整某用户可同时注册的终端（设备）数量上限。
 *
 * 达上限后新终端注册会顶掉最久未活跃的**离线**终端；全部在线则被拒绝。
 * 调小不影响已注册终端，仅限制后续新终端注册。
 */
import { useState } from 'react';
import { Modal, InputNumber, App, Typography, Alert } from 'antd';

import { usersApi } from '@/services/users';
import { ApiError } from '@/services/http';
import type { UserDto } from '@/types/api';

const { Text } = Typography;

interface Props {
  open: boolean;
  user: UserDto | null;
  onClose: () => void;
  onSaved: () => void;
}

export function SetMaxDevicesModal({ open, user, onClose, onSaved }: Props) {
  const { message } = App.useApp();
  const [value, setValue] = useState<number>(1);
  const [saving, setSaving] = useState(false);

  const handleOk = async () => {
    if (!user) return;
    setSaving(true);
    try {
      await usersApi.updateUser(user.id, { maxDevices: value });
      message.success(`已将 ${user.username} 的终端上限设为 ${value}`);
      onSaved();
      onClose();
    } catch (err) {
      message.error(err instanceof ApiError ? err.message || '保存失败' : '网络异常，请稍后再试');
    } finally {
      setSaving(false);
    }
  };

  return (
    <Modal
      open={open}
      title={user ? `设置终端上限 · ${user.username}` : '设置终端上限'}
      afterOpenChange={(visible) => {
        if (visible) setValue(user?.maxDevices ?? 1);
      }}
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
        message="限制该用户可同时注册的终端（设备）数量"
        description="达上限后，新终端注册会顶掉最久未活跃的离线终端；若全部终端在线则注册被拒绝。调小上限不影响已注册终端。"
      />
      <Text type="secondary">终端数量上限（1-100）</Text>
      <InputNumber
        min={1}
        max={100}
        precision={0}
        style={{ width: '100%', marginTop: 6 }}
        value={value}
        onChange={(v) => setValue(v ?? 1)}
      />
    </Modal>
  );
}
