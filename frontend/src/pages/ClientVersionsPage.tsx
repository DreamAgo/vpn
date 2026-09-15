import { useState } from 'react';
import { Alert, App, Button, Card, Descriptions, Input, Space, Switch, Table, Typography } from 'antd';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { clientUpdatesApi } from '@/services/clientUpdates';
const time = (value: number | null | undefined) => value ? new Date(value).toLocaleString() : '尚未同步';
export function ClientVersionsPage() {
  const { message } = App.useApp();
  const cache = useQueryClient();
  const query = useQuery({ queryKey: ['client-updates'], queryFn: clientUpdatesApi.status, refetchInterval: 3000 });
  const [base, setBase] = useState<string | null>(null);
  const [token, setToken] = useState<string | null>(null);
  const [proxy, setProxy] = useState<string | null>(null);
  const [enabled, setEnabled] = useState<boolean | null>(null);
  const [saving, setSaving] = useState(false);
  const [starting, setStarting] = useState(false);
  const data = query.data;
  const address = base ?? (data?.publicBaseUrl || window.location.origin);
  const auto = enabled ?? data?.autoSync ?? false;
  const save = async () => {
    setSaving(true);
    try {
      const next = await clientUpdatesApi.save(auto, address, proxy ?? data?.proxyUrl ?? "", token ?? undefined);
      cache.setQueryData(['client-updates'], next);
      setBase(null); setEnabled(null); setProxy(null); setToken(null);
      message.success('设置已保存');
    } catch (e) { message.error(e instanceof Error ? e.message : '保存失败'); }
    finally { setSaving(false); }
  };
  const sync = async () => {
    setStarting(true);
    try {
      await clientUpdatesApi.sync();
      await query.refetch();
      message.info('开始同步，所有文件校验完成后将自动发布');
    } catch (e) { message.error(e instanceof Error ? e.message : '无法开始同步'); }
    finally { setStarting(false); }
  };
  return <Space direction="vertical" size="middle" style={{ width: '100%', maxWidth: 1100 }}>
    <Typography.Title level={4}>客户端版本</Typography.Title>
    <Typography.Paragraph type="secondary">从 GitHub 同步正式版本，安装包保存在本服务端。客户端检查更新和下载安装包均使用本服务端地址。</Typography.Paragraph>
    {query.isError && <Alert type="error" showIcon message="无法读取版本信息" description={query.error.message} action={<Button onClick={() => query.refetch()}>重试</Button>} />}
    <Card title="同步设置" loading={query.isPending}>
      <Space direction="vertical" style={{ width: '100%' }} size="middle">
        <Typography.Text>来源：{data?.repository || 'DreamAgo/vpn'} · 最新正式发布</Typography.Text>
        <label htmlFor="update-base">客户端可访问的服务端地址</label>
        <Input id="update-base" value={address} onChange={e => setBase(e.target.value)} placeholder="https://vpn.xe-flow.com:8443" disabled={data?.syncing} />
        <Typography.Text type="secondary">客户端须使用此服务端的更新地址。修改登录地址不会自动迁移旧客户端的更新地址。</Typography.Text>
        <label htmlFor="update-proxy">GitHub 下载代理（可选）</label>
        <Input.Password id="update-proxy" value={proxy ?? data?.proxyUrl ?? ''} onChange={e => setProxy(e.target.value)} placeholder="http://代理服务器:7897" autoComplete="off" disabled={data?.syncing || saving} />
        <Typography.Text type="secondary">支持 HTTP/HTTPS 代理，手动和自动同步均生效。留空直连。请填写服务端能访问的地址；127.0.0.1 指服务端自身。</Typography.Text>
        <label htmlFor="update-token">GitHub Token（可选）</label>
        <Input.Password id="update-token" value={token ?? ''} onChange={e => setToken(e.target.value || null)} autoComplete="new-password" placeholder={data?.githubTokenSet ? '已保存，留空保留原 Token' : '填写 GitHub Token'} disabled={data?.syncing || saving} />
        <Space><Typography.Text type="secondary">{token === '' ? '保存后将清除 Token' : data?.githubTokenSet ? 'Token 已配置，保存后不回显' : '用于认证 GitHub API 请求'}</Typography.Text>
          {(data?.githubTokenSet || token) && <Button size="small" onClick={() => setToken('')} disabled={data?.syncing || saving}>清除 Token</Button>}
        </Space>
        <Space><Switch checked={auto} onChange={setEnabled} disabled={data?.syncing} /><span>每小时自动同步（开启后一分钟内首次检查）</span></Space>
        <Space wrap>
          <Button onClick={save} loading={saving} disabled={!data || data.syncing || starting}>保存设置</Button>
          <Button type="primary" onClick={sync} loading={starting || data?.syncing} disabled={!data?.publicBaseUrl || saving || base !== null || enabled !== null || proxy !== null || token !== null}>立即同步最新版本</Button>
        </Space>
        {!data?.publicBaseUrl && <Typography.Text type="secondary">请先确认服务端地址并保存设置，再开始同步。</Typography.Text>}
      </Space>
    </Card>
    {data?.syncing && <Alert type="info" showIcon message="正在下载并校验安装包" description="同步可能需要数分钟。可以离开页面，服务端会继续下载；完成前客户端仍使用原版本。" />}
    {data?.lastError && <Alert type="error" showIcon message="最近一次同步未成功" description={data.lastError} />}
    <Card title="当前发布" loading={query.isPending}>
      <Descriptions column={1}>
        <Descriptions.Item label="客户端版本">{data?.manifest?.version || '尚未发布'}</Descriptions.Item>
        <Descriptions.Item label="最近检查">{time(data?.lastCheckedAt)}</Descriptions.Item>
        <Descriptions.Item label="最近同步成功">{time(data?.lastSyncedAt)}</Descriptions.Item>
        <Descriptions.Item label="下次自动检查">{data?.autoSync ? time(data.nextCheckAt) : '未开启'}</Descriptions.Item>
        <Descriptions.Item label="更新清单">{data?.publicBaseUrl ? <a href={`${data.publicBaseUrl}/updates/latest.json`} target="_blank" rel="noreferrer">查看清单</a> : '先保存服务端地址'}</Descriptions.Item>
      </Descriptions>
      {data?.manifest?.notes && <Typography.Paragraph style={{ whiteSpace: 'pre-wrap', marginTop: 16 }}>{data.manifest.notes}</Typography.Paragraph>}
      <Table rowKey="name" pagination={false} scroll={{ x: 650 }} dataSource={data?.manifest?.downloads?.filter(a => !a.name.endsWith('.sig')) || []} columns={[
        { title: '本地安装包', dataIndex: 'name' },
        { title: '大小', dataIndex: 'size', render: (n: number) => `${(n / 1024 / 1024).toFixed(1)} MB` },
        { title: '下载', dataIndex: 'url', render: (url: string) => <a href={url}>下载安装包</a> },
      ]} />
    </Card>
  </Space>;
}
