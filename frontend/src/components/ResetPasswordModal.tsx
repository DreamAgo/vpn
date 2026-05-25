/**
 * Story 3.9：重置密码结果弹窗。
 *
 * 展示新生成的密码（明文 monospace）+ 一键复制 + 仅显示一次提示。
 */
import { useState } from 'react';
import { Modal, Button, Typography, Space, Alert, App, Input } from 'antd';
import { CheckOutlined, CopyOutlined } from '@ant-design/icons';

import { codeFontFamily } from '@/theme';
import { useCopyToClipboard } from '@/hooks/useCopyToClipboard';

const { Text } = Typography;

interface Props {
  open: boolean;
  onClose: () => void;
  newPassword: string;
}

export function ResetPasswordModal({ open, onClose, newPassword }: Props) {
  const { message } = App.useApp();
  const { copied, copy, supported } = useCopyToClipboard();
  const [showFallback, setShowFallback] = useState(false);

  const handleCopy = async () => {
    const ok = await copy(newPassword);
    if (ok) {
      message.success('密码已复制');
    } else {
      setShowFallback(true);
    }
  };

  return (
    <Modal
      open={open}
      onCancel={onClose}
      title="密码已重置"
      maskClosable={false}
      footer={[
        <Button key="done" type="primary" onClick={onClose}>
          完成
        </Button>,
      ]}
    >
      <Space direction="vertical" size="middle" style={{ width: '100%' }}>
        <Alert
          type="warning"
          showIcon
          message="该密码仅显示一次，请立即发给用户"
        />
        <div>
          <Text type="secondary">新密码</Text>
          <div
            style={{
              fontFamily: codeFontFamily,
              fontSize: 16,
              wordBreak: 'break-all',
              marginTop: 4,
            }}
          >
            {newPassword}
          </div>
        </div>

        {!supported || showFallback ? (
          <div>
            <Text type="warning">无法自动复制，请手动复制：</Text>
            <Input.TextArea
              readOnly
              value={newPassword}
              autoSize={{ minRows: 1, maxRows: 3 }}
              style={{ fontFamily: codeFontFamily, marginTop: 8 }}
            />
          </div>
        ) : (
          <Button
            icon={copied ? <CheckOutlined /> : <CopyOutlined />}
            onClick={handleCopy}
            block
          >
            {copied ? '已复制' : '复制密码'}
          </Button>
        )}
      </Space>
    </Modal>
  );
}
