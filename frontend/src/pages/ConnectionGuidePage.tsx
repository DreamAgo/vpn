/**
 * Story 4.18：接入向导页（面向普通登录用户）。
 *
 * - 分步说明如何用 CLI 客户端接入（下载安装 → vpn-cli login → vpn-cli up）。
 * - 提供下载 vpn.conf 按钮（fetch GET /peers/me/config，text/plain，Blob 触发浏览器下载）。
 * - 复用 useCopyToClipboard 提供"复制配置"。
 *
 * 注意：/peers/me/config 返回 text/plain（非 ApiResponse 信封），
 * 因此不能走 http 实例（其响应拦截器会按信封解析），改用原生 fetch + 内存中的 access token。
 */
import { useState } from 'react';
import { Card, Steps, Typography, Button, Space, App, Alert } from 'antd';
import { DownloadOutlined, CopyOutlined, CheckOutlined } from '@ant-design/icons';

import { codeFontFamily } from '@/theme';
import { getAccessToken } from '@/services/http';
import { useCopyToClipboard } from '@/hooks/useCopyToClipboard';

const { Title, Paragraph, Text } = Typography;

const CONFIG_URL = '/api/v1/peers/me/config';

function CodeBlock({ children }: { children: string }) {
  return (
    <pre
      style={{
        fontFamily: codeFontFamily,
        fontSize: 13,
        background: '#001529',
        color: '#E6F4FF',
        padding: '12px 16px',
        borderRadius: 4,
        margin: '8px 0 0',
        overflowX: 'auto',
      }}
    >
      {children}
    </pre>
  );
}

export function ConnectionGuidePage() {
  const { message } = App.useApp();
  const { copied, copy, supported } = useCopyToClipboard();
  const [downloading, setDownloading] = useState(false);
  const [copying, setCopying] = useState(false);

  /** 拉取 vpn.conf 文本（带 Authorization）。 */
  const fetchConfig = async (): Promise<string> => {
    const token = getAccessToken();
    const res = await fetch(CONFIG_URL, {
      headers: token ? { Authorization: `Bearer ${token}` } : undefined,
      credentials: 'include',
    });
    if (!res.ok) {
      throw new Error(`下载失败（HTTP ${res.status}）`);
    }
    return res.text();
  };

  const handleDownload = async () => {
    setDownloading(true);
    try {
      const text = await fetchConfig();
      const blob = new Blob([text], { type: 'text/plain;charset=utf-8' });
      const url = URL.createObjectURL(blob);
      const a = document.createElement('a');
      a.href = url;
      a.download = 'vpn.conf';
      document.body.appendChild(a);
      a.click();
      document.body.removeChild(a);
      URL.revokeObjectURL(url);
      message.success('已下载 vpn.conf');
    } catch (err) {
      message.error(err instanceof Error ? err.message : '下载配置失败');
    } finally {
      setDownloading(false);
    }
  };

  const handleCopyConfig = async () => {
    setCopying(true);
    try {
      const text = await fetchConfig();
      const ok = await copy(text);
      if (ok) {
        message.success('已复制配置内容');
      } else {
        message.warning('当前环境无法自动复制，请改用下载');
      }
    } catch (err) {
      message.error(err instanceof Error ? err.message : '获取配置失败');
    } finally {
      setCopying(false);
    }
  };

  return (
    <div style={{ padding: 24, maxWidth: 880 }}>
      <Title level={4} style={{ marginBottom: 16 }}>
        接入指南
      </Title>

      <Alert
        type="info"
        showIcon
        style={{ marginBottom: 16 }}
        message="按以下步骤使用命令行客户端（vpn-cli）接入虚拟网络。"
      />

      <Card>
        <Steps
          direction="vertical"
          current={-1}
          items={[
            {
              title: '下载并安装客户端',
              description: (
                <div>
                  <Paragraph style={{ marginBottom: 4 }}>
                    在你的设备上安装 <Text code>vpn-cli</Text> 与 WireGuard。
                  </Paragraph>
                  <Text type="secondary">macOS / Linux 示例：</Text>
                  <CodeBlock>{'# macOS\nbrew install wireguard-tools\n\n# Debian / Ubuntu\nsudo apt install wireguard-tools'}</CodeBlock>
                </div>
              ),
            },
            {
              title: '登录账号',
              description: (
                <div>
                  <Paragraph style={{ marginBottom: 4 }}>
                    使用管理员分配的用户名 / 密码登录：
                  </Paragraph>
                  <CodeBlock>{'vpn-cli login'}</CodeBlock>
                </div>
              ),
            },
            {
              title: '建立连接',
              description: (
                <div>
                  <Paragraph style={{ marginBottom: 4 }}>
                    登录成功后，一条命令即可接入：
                  </Paragraph>
                  <CodeBlock>{'vpn-cli up'}</CodeBlock>
                  <Paragraph type="secondary" style={{ marginTop: 8, marginBottom: 0 }}>
                    断开连接使用 <Text code>vpn-cli down</Text>。
                  </Paragraph>
                </div>
              ),
            },
            {
              title: '（可选）手动导入配置',
              description: (
                <div>
                  <Paragraph style={{ marginBottom: 8 }}>
                    若使用 WireGuard 图形客户端，可下载配置文件后手动导入：
                  </Paragraph>
                  <Space wrap>
                    <Button
                      type="primary"
                      icon={<DownloadOutlined />}
                      loading={downloading}
                      onClick={handleDownload}
                    >
                      下载 vpn.conf
                    </Button>
                    {supported && (
                      <Button
                        icon={copied ? <CheckOutlined /> : <CopyOutlined />}
                        loading={copying}
                        onClick={handleCopyConfig}
                      >
                        {copied ? '已复制' : '复制配置'}
                      </Button>
                    )}
                  </Space>
                </div>
              ),
            },
          ]}
        />
      </Card>
    </div>
  );
}
