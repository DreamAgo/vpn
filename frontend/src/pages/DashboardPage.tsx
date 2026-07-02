import { useState } from 'react';
import { Card, Col, Row, Descriptions, Tag, Skeleton, Alert, Typography, List, Space, Button } from 'antd';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useNavigate } from 'react-router-dom';
import dayjs from 'dayjs';
import relativeTime from 'dayjs/plugin/relativeTime';
import 'dayjs/locale/zh-cn';

import { systemApi } from '@/services/auth';
import { usersApi } from '@/services/users';
import { peersApi } from '@/services/peers';
import { NodeStatusDot } from '@/components/NodeStatusDot';
import { EditServerRoutesModal } from '@/components/EditServerRoutesModal';
import { PeerHealthTable, PeerEventsCard } from '@/components/PeerHealthSection';
import type { AdminPeerView } from '@/types/api';

dayjs.extend(relativeTime);
dayjs.locale('zh-cn');

const { Text, Link } = Typography;
const OFFLINE_ALERT_MINUTES = 10;

function SectionHead({ label, title, extra }: { label: string; title: string; extra?: React.ReactNode }) {
  return (
    <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 12 }}>
      <div>
        <span className="bp-eyebrow">{label}</span>
        <div style={{ fontSize: 18, fontWeight: 700, marginTop: 6, color: 'var(--ink)' }}>
          {title}
        </div>
      </div>
      {extra}
    </div>
  );
}

function Tile({ eyebrow, value, caption, accent }: { eyebrow: string; value: React.ReactNode; caption: string; accent?: string }) {
  return (
    <div
      style={{
        background: 'var(--card)',
        border: '1px solid var(--line)',
        borderRadius: 8,
        padding: '18px 20px',
        height: '100%',
        boxShadow: 'var(--surface-shadow)',
      }}
    >
      <span className="bp-eyebrow">{eyebrow}</span>
      <div style={{ fontFamily: 'var(--font-mono)', fontWeight: 700, fontSize: 32, lineHeight: 1.1, marginTop: 14, color: accent ?? 'var(--ink)' }}>
        {value}
      </div>
      <div style={{ fontSize: 12, color: 'var(--ink-soft)', marginTop: 2 }}>{caption}</div>
    </div>
  );
}

export function DashboardPage() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [editRoutes, setEditRoutes] = useState(false);

  const { data: system, isLoading: sysLoading, isError: sysError, error: sysErr } = useQuery({
    queryKey: ['system-info'],
    queryFn: () => systemApi.getSystemInfo(),
  });
  const { data: usersPage } = useQuery({
    queryKey: ['dashboard-users-total'],
    queryFn: () => usersApi.listUsers({ page: 1, pageSize: 1 }),
  });
  const { data: peersPage } = useQuery({
    queryKey: ['dashboard-peers'],
    queryFn: () => peersApi.listAdminPeers({ page: 1, pageSize: 500 }),
    refetchInterval: 10_000,
  });

  const peers: AdminPeerView[] = peersPage?.items ?? [];
  const onlineCount = peers.filter((p) => p.status === 'online').length;
  const offlineCount = peers.filter((p) => p.status === 'offline').length;
  const todayThreshold = dayjs().subtract(24, 'hour').valueOf();
  const newTodayCount = peers.filter((p) => p.createdAt >= todayThreshold).length;

  const now = dayjs();
  const staleOffline = peers
    .filter(
      (p) =>
        p.status === 'offline' &&
        p.lastSeenAt != null &&
        now.diff(dayjs(p.lastSeenAt), 'minute') >= OFFLINE_ALERT_MINUTES
    )
    .sort((a, b) => (a.lastSeenAt ?? 0) - (b.lastSeenAt ?? 0));

  const recentPeers = [...peers]
    .filter((p) => p.lastSeenAt != null)
    .sort((a, b) => (b.lastSeenAt ?? 0) - (a.lastSeenAt ?? 0))
    .slice(0, 10);

  return (
    <div>
      <div style={{ marginBottom: 22 }}>
        <span className="bp-eyebrow">控制总览</span>
        <div style={{ fontSize: 28, fontWeight: 700, marginTop: 8, color: 'var(--ink)' }}>
          仪表盘
        </div>
      </div>

      {/* 离线告警 */}
      {staleOffline.length > 0 && (
        <Space direction="vertical" style={{ width: '100%', marginBottom: 18 }} size="small">
          {staleOffline.slice(0, 5).map((p) => (
            <Alert
              key={p.id}
              type="warning"
              showIcon
              message={`节点 ${p.deviceName}（${p.username}）已离线 ${now.diff(dayjs(p.lastSeenAt), 'minute')} 分钟`}
              action={
                <a onClick={() => navigate('/peers')} style={{ whiteSpace: 'nowrap' }}>
                  处理
                </a>
              }
            />
          ))}
        </Space>
      )}

      {/* KPI 瓦片 */}
      <Row gutter={[16, 16]} style={{ marginBottom: 26 }}>
        <Col xs={12} lg={6}>
          <Tile eyebrow="用户" value={usersPage?.total ?? '—'} caption="用户总数" />
        </Col>
        <Col xs={12} lg={6}>
          <Tile eyebrow="在线" value={peersPage ? onlineCount : '—'} caption="在线节点" accent="#16a34a" />
        </Col>
        <Col xs={12} lg={6}>
          <Tile eyebrow="离线" value={peersPage ? offlineCount : '—'} caption="离线节点" accent="#64748b" />
        </Col>
        <Col xs={12} lg={6}>
          <Tile eyebrow="24 小时" value={peersPage ? newTodayCount : '—'} caption="今日新增" accent="#d97706" />
        </Col>
      </Row>

      <Row gutter={[20, 20]}>
        {/* 最近节点 */}
        <Col xs={24} lg={12}>
          <SectionHead
            label="活动"
            title="最近活动节点"
            extra={<Link onClick={() => navigate('/peers')}>查看全部</Link>}
          />
          <Card styles={{ body: { padding: recentPeers.length === 0 ? 20 : 4 } }}>
            {recentPeers.length === 0 ? (
              <Text type="secondary">暂无活动节点</Text>
            ) : (
              <List
                size="small"
                dataSource={recentPeers}
                renderItem={(p) => (
                  <List.Item>
                    <Space style={{ width: '100%', justifyContent: 'space-between' }}>
                      <Space size="middle">
                        <NodeStatusDot status={p.status} size="sm" />
                        <Text strong>{p.deviceName}</Text>
                        <Text code>{p.vpnIp}</Text>
                      </Space>
                      <Text type="secondary" style={{ fontFamily: 'var(--font-mono)', fontSize: 12 }}>
                        {p.lastSeenAt ? dayjs(p.lastSeenAt).fromNow() : '从未'}
                      </Text>
                    </Space>
                  </List.Item>
                )}
              />
            )}
          </Card>
        </Col>

        {/* 系统信息 */}
        <Col xs={24} lg={12}>
          <SectionHead
            label="系统"
            title="系统信息"
            extra={
              system ? (
                <Button type="link" size="small" onClick={() => setEditRoutes(true)}>
                  编辑 LAN 网段
                </Button>
              ) : undefined
            }
          />
          <Card styles={{ body: { padding: sysLoading ? 20 : 0 } }}>
            {sysError && (
              <Alert
                style={{ margin: 16 }}
                type="error"
                showIcon
                message="无法加载系统信息"
                description={sysErr instanceof Error ? sysErr.message : '未知错误'}
              />
            )}
            {sysLoading && <Skeleton active paragraph={{ rows: 4 }} />}
            {system && (
              <Descriptions column={1} bordered size="small">
                <Descriptions.Item label="版本">
                  <Text code>v{system.version}</Text>
                </Descriptions.Item>
                <Descriptions.Item label="监听端口">
                  <Text code>{system.listenPort}</Text>
                </Descriptions.Item>
                <Descriptions.Item label="VPN 子网">
                  <Text code>{system.vpnSubnet}</Text>
                </Descriptions.Item>
                <Descriptions.Item label="服务端 Endpoint">
                  <Text code>{system.serverEndpoint}</Text>
                </Descriptions.Item>
                <Descriptions.Item label="服务端公钥">
                  <Typography.Text code copyable={{ text: system.serverPublicKey }} style={{ wordBreak: 'break-all' }}>
                    {system.serverPublicKey}
                  </Typography.Text>
                </Descriptions.Item>
                <Descriptions.Item label="服务端 LAN 网段">
                  {(system.serverRoutes ?? []).length > 0 ? (
                    <Space size={[4, 4]} wrap>
                      {system.serverRoutes!.map((r) => (
                        <Tag key={r} color="blue">
                          {r}
                        </Tag>
                      ))}
                    </Space>
                  ) : (
                    <Text type="secondary">未配置</Text>
                  )}
                </Descriptions.Item>
                <Descriptions.Item label="启动时间">
                  <Text code>{dayjs(system.startedAt).format('YYYY-MM-DD HH:mm:ss')}</Text>
                </Descriptions.Item>
              </Descriptions>
            )}
          </Card>
        </Col>
      </Row>

      {/* 节点健康监控 */}
      <Row gutter={[20, 20]} style={{ marginTop: 20 }}>
        <Col xs={24} xl={16}>
          <SectionHead label="健康" title="节点健康" />
          <Card styles={{ body: { padding: 4 } }}>
            <PeerHealthTable peers={peers} />
          </Card>
        </Col>
        <Col xs={24} xl={8}>
          <SectionHead label="变更" title="最近变更记录" />
          <PeerEventsCard />
        </Col>
      </Row>

      {system && (
        <EditServerRoutesModal
          open={editRoutes}
          current={system.serverRoutes ?? []}
          onClose={() => setEditRoutes(false)}
          onSaved={() => queryClient.invalidateQueries({ queryKey: ['system-info'] })}
        />
      )}
    </div>
  );
}
