/**
 * 接入指南（面向普通登录用户）。
 *
 * 三种接入方式：
 *  1. macOS 桌面端 VPN Desktop（图形界面，推荐）
 *  2. 命令行 vpn-cli（全平台单二进制，自带 WireGuard，零安装）
 *  3.（可选）标准 WireGuard 客户端 + 导入 vpn.conf
 *
 * 配置下载：/peers/me/config 返回 text/plain（非 ApiResponse 信封），
 * 故不能走 http 实例（其响应拦截器按信封解析），改用原生 fetch + 内存 access token。
 */
import { useState } from 'react';
import { Card, Typography, Button, Space, App, Tag } from 'antd';
import {
  DownloadOutlined,
  CopyOutlined,
  CheckOutlined,
  AppleOutlined,
  CodeOutlined,
  ApiOutlined,
} from '@ant-design/icons';

import { getAccessToken } from '@/services/http';
import { useCopyToClipboard } from '@/hooks/useCopyToClipboard';

const { Paragraph, Text } = Typography;

const CONFIG_URL = '/api/v1/peers/me/config';
// 接入用的服务端地址 = 当前控制台地址（用户从哪登录就连哪）。
const SERVER = typeof window !== 'undefined' ? window.location.origin : 'https://vpn.example.com';

function CodeBlock({ children }: { children: string }) {
  const lines = children.split('\n');
  return (
    <pre
      style={{
        fontFamily: 'var(--font-mono)',
        fontSize: 12.5,
        lineHeight: 1.75,
        background: '#0f172a',
        border: '1px solid #1e293b',
        borderRadius: 8,
        color: '#e2e8f0',
        padding: '14px 16px',
        margin: '10px 0 0',
        overflowX: 'auto',
      }}
    >
      {lines.map((ln, i) => {
        const isComment = ln.trimStart().startsWith('#');
        const isBlank = ln.trim() === '';
        return (
          <div
            key={i}
            style={{ color: isComment ? '#94a3b8' : '#e2e8f0', minHeight: '1.75em' }}
          >
            {!isComment && !isBlank && (
              <span style={{ color: '#60a5fa', userSelect: 'none' }}>$ </span>
            )}
            {ln || ' '}
          </div>
        );
      })}
    </pre>
  );
}

/** 小节标题块：序号眉题 + 图标 + 标题（+ 可选徽标）。 */
function MethodHead({
  index,
  icon,
  title,
  badge,
}: {
  index: string;
  icon: React.ReactNode;
  title: string;
  badge?: string;
}) {
  return (
    <div style={{ marginBottom: 4 }}>
      <span className="bp-eyebrow">{index}</span>
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 10,
          marginTop: 10,
          fontSize: 18,
          fontWeight: 700,
          color: 'var(--ink)',
        }}
      >
        <span style={{ color: 'var(--accent)', display: 'inline-flex', fontSize: 18 }}>{icon}</span>
        {title}
        {badge && (
          <Tag
            color="blue"
            style={{
              fontSize: 12,
              marginInlineStart: 4,
            }}
          >
            {badge}
          </Tag>
        )}
      </div>
    </div>
  );
}

/** 编号步骤行。 */
function Step({ n, children }: { n: number; children: React.ReactNode }) {
  return (
    <div style={{ display: 'flex', gap: 12, marginTop: 16 }}>
      <span
        style={{
          flex: 'none',
          width: 22,
          height: 22,
          borderRadius: '50%',
          border: '1px solid var(--line-strong)',
          color: 'var(--accent)',
          fontWeight: 600,
          fontSize: 11,
          display: 'inline-flex',
          alignItems: 'center',
          justifyContent: 'center',
          marginTop: 1,
        }}
      >
        {n}
      </span>
      <div style={{ flex: 1, minWidth: 0 }}>{children}</div>
    </div>
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
    <div style={{ maxWidth: 900 }}>
      <div style={{ marginBottom: 18 }}>
        <span className="bp-eyebrow">接入说明</span>
        <div
          style={{
            fontSize: 28,
            fontWeight: 700,
            marginTop: 8,
            color: 'var(--ink)',
          }}
        >
          接入指南
        </div>
        <Paragraph style={{ color: 'var(--ink-soft)', marginTop: 8, marginBottom: 0 }}>
          客户端为<Text style={{ color: 'var(--accent)' }}>零安装单程序</Text>，内置 WireGuard 数据面——
          <Text style={{ color: 'var(--ink)' }}>无需</Text>另装 <Text code>wireguard-tools</Text> 等任何依赖。
          用管理员分配的账号登录即可接入。
        </Paragraph>
      </div>

      <Space direction="vertical" size={18} style={{ width: '100%' }}>
        {/* 方式一：macOS 桌面端 */}
        <Card>
          <MethodHead index="METHOD 01" icon={<AppleOutlined />} title="macOS 桌面端 VPN Desktop" badge="推荐" />
          <Step n={1}>
            <Paragraph style={{ color: 'var(--ink-soft)', margin: 0 }}>
              向管理员获取 <Text code>VPN Desktop.app</Text>，拖入「应用程序」后打开（菜单栏与程序坞均有图标）。
            </Paragraph>
          </Step>
          <Step n={2}>
            <Paragraph style={{ color: 'var(--ink-soft)', margin: 0 }}>
              在登录框填入服务端地址与账号：
            </Paragraph>
            <CodeBlock>{`服务端地址：${SERVER}\n用户名 / 密码：管理员分配`}</CodeBlock>
          </Step>
          <Step n={3}>
            <Paragraph style={{ color: 'var(--ink-soft)', margin: 0 }}>
              点「连接」即接入。断线自动重连，状态变化有系统通知；点「断开」即下线。
            </Paragraph>
          </Step>
        </Card>

        {/* 方式二：命令行 vpn-cli */}
        <Card>
          <MethodHead index="METHOD 02" icon={<CodeOutlined />} title="命令行 vpn-cli（全平台 / 服务器节点）" />
          <Step n={1}>
            <Paragraph style={{ color: 'var(--ink-soft)', margin: 0 }}>
              获取 <Text code>vpn-cli</Text> 单个可执行文件（macOS / Linux / Windows）。无需安装其它依赖。
            </Paragraph>
          </Step>
          <Step n={2}>
            <Paragraph style={{ color: 'var(--ink-soft)', margin: 0 }}>登录（按提示安全输入用户名 / 密码）：</Paragraph>
            <CodeBlock>{`vpn-cli login --server ${SERVER}`}</CodeBlock>
          </Step>
          <Step n={3}>
            <Paragraph style={{ color: 'var(--ink-soft)', margin: 0 }}>
              启动后台守护进程（需 root / 管理员——用于开启 TUN 设备）：
            </Paragraph>
            <CodeBlock>{`# 安装为系统服务（开机自启）\nsudo vpn-cli daemon install\n\n# 或仅前台运行一次\nsudo vpn-cli daemon run`}</CodeBlock>
          </Step>
          <Step n={4}>
            <Paragraph style={{ color: 'var(--ink-soft)', margin: 0 }}>连接 / 断开 / 查看状态：</Paragraph>
            <CodeBlock>{`vpn-cli up\nvpn-cli status\nvpn-cli down`}</CodeBlock>
          </Step>
          <Step n={5}>
            <Paragraph style={{ color: 'var(--ink-soft)', margin: 0 }}>
              <Text style={{ color: 'var(--ink)' }}>站点网关（异地组网）</Text>：登录时用 <Text code>--route</Text>
              声明本机背后的 LAN 网段，其它节点即可经本机访问该内网：
            </Paragraph>
            <CodeBlock>{`vpn-cli login --server ${SERVER} --route 192.168.10.0/24`}</CodeBlock>
          </Step>
        </Card>

        {/* 方式三：标准 WireGuard 客户端 */}
        <Card>
          <MethodHead index="METHOD 03" icon={<ApiOutlined />} title="标准 WireGuard 客户端（可选）" />
          <Paragraph style={{ color: 'var(--ink-soft)', marginTop: 14, marginBottom: 12 }}>
            若偏好官方 WireGuard 图形客户端，可下载配置文件后手动导入。注意：此方式为静态配置，
            不随用户组 / 网段变更自动更新路由。
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
        </Card>
      </Space>
    </div>
  );
}
