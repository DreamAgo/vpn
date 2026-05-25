/**
 * Story 3.8：分享接入链接 + 初始密码弹窗。
 *
 * 创建用户成功后展示，便于管理员一键复制后发给最终用户。
 * - 显示接入链接 + 初始密码（明文 monospace）。
 * - 一键复制多行文本（链接 + 用户名 + 密码 + 简单说明）。
 * - clipboard 不可用时降级为只读 textarea + 手动复制提示。
 */
import { useMemo, useState } from 'react';
import { Modal, Button, Typography, Space, App, Input } from 'antd';
import { CheckOutlined, CopyOutlined } from '@ant-design/icons';

import { codeFontFamily } from '@/theme';
import { useCopyToClipboard } from '@/hooks/useCopyToClipboard';

const { Text } = Typography;

interface Props {
  open: boolean;
  onClose: () => void;
  link?: string;
  password: string;
  username: string;
}

export function ShareLinkModal({ open, onClose, link, password, username }: Props) {
  const { message } = App.useApp();
  const { copied, copy, supported } = useCopyToClipboard();
  const [showFallback, setShowFallback] = useState(false);

  const accessLink = link || `${window.location.origin}/setup`;

  const shareText = useMemo(
    () =>
      [
        '【VPN 接入信息】',
        `接入链接：${accessLink}`,
        `用户名：${username}`,
        `初始密码：${password}`,
        '',
        '使用说明：打开接入链接，使用上述用户名和初始密码登录，首次登录后请立即修改密码。',
      ].join('\n'),
    [accessLink, username, password]
  );

  const handleCopy = async () => {
    const ok = await copy(shareText);
    if (ok) {
      message.success('已复制链接和密码');
    } else {
      setShowFallback(true);
    }
  };

  return (
    <Modal
      open={open}
      onCancel={onClose}
      title="用户创建成功"
      maskClosable={false}
      footer={[
        <Button key="done" type="primary" onClick={onClose}>
          完成
        </Button>,
      ]}
    >
      <Space direction="vertical" size="middle" style={{ width: '100%' }}>
        <div>
          <Text type="secondary">接入链接</Text>
          <div style={{ fontFamily: codeFontFamily, wordBreak: 'break-all', marginTop: 4 }}>
            {accessLink}
          </div>
        </div>
        <div>
          <Text type="secondary">初始密码</Text>
          <div
            style={{
              fontFamily: codeFontFamily,
              fontSize: 16,
              wordBreak: 'break-all',
              marginTop: 4,
            }}
          >
            {password}
          </div>
        </div>

        {!supported || showFallback ? (
          <div>
            <Text type="warning">无法自动复制，请手动复制以下内容：</Text>
            <Input.TextArea
              readOnly
              value={shareText}
              autoSize={{ minRows: 5, maxRows: 8 }}
              style={{ fontFamily: codeFontFamily, marginTop: 8 }}
            />
          </div>
        ) : (
          <Button
            icon={copied ? <CheckOutlined /> : <CopyOutlined />}
            onClick={handleCopy}
            block
          >
            {copied ? '已复制' : '复制链接和密码'}
          </Button>
        )}
      </Space>
    </Modal>
  );
}
