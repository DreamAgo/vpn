import { useState } from 'react';
import { App, Alert, Button, DatePicker, Modal, Space } from 'antd';
import dayjs from 'dayjs';
import utc from 'dayjs/plugin/utc';
import timezone from 'dayjs/plugin/timezone';
import { usersApi } from '@/services/users';

dayjs.extend(utc);
dayjs.extend(timezone);

export function GrantExpiryButton({ userId, groupId, groupName, expiresAt, onSaved }: {
  userId: string; groupId: string; groupName: string; expiresAt: number; onSaved: () => void;
}) {
  const { message } = App.useApp();
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [value, setValue] = useState<dayjs.Dayjs | null>(null);
  const [expected, setExpected] = useState(expiresAt);
  const [comparisonTime, setComparisonTime] = useState(() => Date.now());
  const save = async () => {
    if (!value) return;
    setBusy(true);
    try {
      // 将选择器显示的墙上时间明确解释为上海时间，不受浏览器时区影响。
      const next = dayjs.tz(value.format('YYYY-MM-DD HH:mm:ss'), 'Asia/Shanghai').valueOf();
      await usersApi.updateGrantExpiry(userId, groupId, next, expected);
      message.success('授权到期时间已更新');
      setOpen(false); onSaved();
    } catch (error) {
      message.error(error instanceof Error ? error.message : '保存失败');
    } finally { setBusy(false); }
  };
  return <>
    <Button type="link" size="small" onClick={() => {
      setComparisonTime(Date.now());
      setExpected(expiresAt);
      setValue(dayjs.utc(dayjs(expiresAt).tz('Asia/Shanghai').format('YYYY-MM-DD HH:mm:ss')));
      setOpen(true);
    }}>修改到期时间</Button>
    <Modal title={`修改授权到期时间 · ${groupName}`} open={open}
      onCancel={() => { if (!busy) setOpen(false); }} onOk={save}
      confirmLoading={busy} okButtonProps={{ disabled: !value }}
      cancelButtonProps={{ disabled: busy }} closable={!busy} maskClosable={!busy} destroyOnHidden>
      <Space direction="vertical" style={{ width: '100%' }}>
        <Alert type="info" showIcon message="上海时间（UTC+8），到达此时刻即失效。此组的已有审批授权会一起更新；其他组及手工分组不变。后续新审批仍按审批期限生效。" />
        <DatePicker aria-label="授权到期时间（上海）" showTime format="YYYY-MM-DD HH:mm:ss"
          value={value} onChange={(next) => { setValue(next); setComparisonTime(Date.now()); }} disabled={busy} style={{ width: '100%' }} />
        {value && dayjs.tz(value.format('YYYY-MM-DD HH:mm:ss'), 'Asia/Shanghai').valueOf() <= comparisonTime
          && <Alert type="warning" showIcon message="所选时间已过去，保存后该组审批授权立即失效。" />}
      </Space>
    </Modal>
  </>;
}
