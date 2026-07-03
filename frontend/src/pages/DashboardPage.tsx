import { useMemo, useState } from 'react';
import {
  Alert,
  Button,
  Card,
  Col,
  Empty,
  List,
  Row,
  Skeleton,
  Space,
  Tag,
  Tooltip,
  Typography,
} from 'antd';
import {
  ApiOutlined,
  BellOutlined,
  ClusterOutlined,
  ExclamationCircleOutlined,
  FieldTimeOutlined,
  GlobalOutlined,
  SafetyCertificateOutlined,
  TeamOutlined,
} from '@ant-design/icons';
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

const { Text, Title } = Typography;
const OFFLINE_ALERT_MINUTES = 10;

function SectionHead({
  eyebrow,
  title,
  extra,
}: {
  eyebrow: string;
  title: string;
  extra?: React.ReactNode;
}) {
  return (
    <div className="dashboard-section-head">
      <div>
        <span className="bp-eyebrow">{eyebrow}</span>
        <div className="dashboard-section-title">{title}</div>
      </div>
      {extra}
    </div>
  );
}

function MetricCard({
  label,
  value,
  note,
  icon,
  tone = 'neutral',
}: {
  label: string;
  value: React.ReactNode;
  note: string;
  icon: React.ReactNode;
  tone?: 'neutral' | 'green' | 'amber' | 'blue' | 'red';
}) {
  return (
    <Card className={`dashboard-metric dashboard-metric-${tone}`}>
      <div className="dashboard-metric-top">
        <span>{label}</span>
        <div className="dashboard-metric-icon">{icon}</div>
      </div>
      <div className="dashboard-metric-value">{value}</div>
      <div className="dashboard-metric-note">{note}</div>
    </Card>
  );
}

function compactPercent(numerator: number, denominator: number): number {
  if (denominator <= 0) return 0;
  return Math.round((numerator / denominator) * 100);
}

function peerLastSeenLabel(peer: AdminPeerView): string {
  if (!peer.lastSeenAt) return '从未';
  return dayjs(peer.lastSeenAt).fromNow();
}

function GatewayTopology({
  peers,
  serverRoutes,
}: {
  peers: AdminPeerView[];
  serverRoutes: string[];
}) {
  const gateways = peers.filter((p) => (p.routedSubnets ?? []).length > 0);
  const clients = peers.filter((p) => (p.routedSubnets ?? []).length === 0);

  return (
    <Card className="dashboard-panel dashboard-topology">
      <div className="topology-root">
        <div className="topology-node topology-server">
          <div className="topology-node-icon">
            <SafetyCertificateOutlined />
          </div>
          <div>
            <Text strong>易链服务端</Text>
            <div className="topology-node-note">
              {serverRoutes.length > 0 ? `${serverRoutes.length} 条服务端 LAN` : '未配置服务端 LAN'}
            </div>
          </div>
        </div>

        <div className="topology-lanes">
          <div className="topology-lane">
            <div className="topology-lane-title">接入终端</div>
            <div className="topology-stack">
              {clients.length === 0 ? (
                <div className="topology-empty">暂无普通终端</div>
              ) : (
                clients.slice(0, 6).map((peer) => (
                  <div key={peer.id} className="topology-chip">
                    <NodeStatusDot status={peer.status} />
                    <span>{peer.deviceName}</span>
                  </div>
                ))
              )}
              {clients.length > 6 && <div className="topology-more">+{clients.length - 6}</div>}
            </div>
          </div>

          <div className="topology-lane topology-gateway-lane">
            <div className="topology-lane-title">站点网关</div>
            <div className="topology-gateways">
              {gateways.length === 0 ? (
                <div className="topology-empty">暂无站点网关</div>
              ) : (
                gateways.map((gateway) => (
                  <div key={gateway.id} className="topology-gateway">
                    <div className="topology-gateway-head">
                      <Space size={6}>
                        <NodeStatusDot status={gateway.status} />
                        <Text strong>{gateway.deviceName}</Text>
                      </Space>
                      <Tag color={gateway.status === 'online' ? 'success' : 'default'}>
                        {gateway.status === 'online' ? '在线' : '离线'}
                      </Tag>
                    </div>
                    <Text type="secondary" className="topology-owner">
                      {gateway.username} · {gateway.vpnIp}
                    </Text>
                    <div className="topology-routes">
                      {(gateway.routedSubnets ?? []).map((route) => (
                        <Tag key={`${gateway.id}-${route}`} color="blue">
                          {route}
                        </Tag>
                      ))}
                    </div>
                  </div>
                ))
              )}
            </div>
          </div>

          <div className="topology-lane">
            <div className="topology-lane-title">服务端 LAN</div>
            <div className="topology-stack">
              {serverRoutes.length === 0 ? (
                <div className="topology-empty">未配置</div>
              ) : (
                serverRoutes.map((route) => (
                  <div key={route} className="topology-chip topology-route-chip">
                    <GlobalOutlined />
                    <span>{route}</span>
                  </div>
                ))
              )}
            </div>
          </div>
        </div>
      </div>
    </Card>
  );
}

export function DashboardPage() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [editRoutes, setEditRoutes] = useState(false);

  const {
    data: system,
    isLoading: sysLoading,
    isError: sysError,
    error: sysErr,
  } = useQuery({
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
  const gatewayCount = peers.filter((p) => (p.routedSubnets ?? []).length > 0).length;
  const routeCount =
    (system?.serverRoutes ?? []).length +
    peers.reduce((sum, peer) => sum + (peer.routedSubnets ?? []).length, 0);
  const todayThreshold = dayjs().subtract(24, 'hour').valueOf();
  const newTodayCount = peers.filter((p) => p.createdAt >= todayThreshold).length;
  const healthScore = compactPercent(onlineCount, peers.length);

  const now = dayjs();
  const staleOfflineGateways = useMemo(
    () =>
      peers
        .filter(
          (p) =>
            (p.routedSubnets ?? []).length > 0 &&
            p.status === 'offline' &&
            p.lastSeenAt != null &&
            now.diff(dayjs(p.lastSeenAt), 'minute') >= OFFLINE_ALERT_MINUTES
        )
        .sort((a, b) => (a.lastSeenAt ?? 0) - (b.lastSeenAt ?? 0)),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [peersPage]
  );

  const recentPeers = useMemo(
    () =>
      [...peers]
        .filter((p) => p.lastSeenAt != null)
        .sort((a, b) => (b.lastSeenAt ?? 0) - (a.lastSeenAt ?? 0))
        .slice(0, 8),
    [peers]
  );

  const topRisk = staleOfflineGateways[0];
  const headlineStatus =
    staleOfflineGateways.length > 0
      ? `${staleOfflineGateways.length} 个站点网关离线超过 ${OFFLINE_ALERT_MINUTES} 分钟`
      : peers.length > 0
        ? '接入面运行正常'
        : '等待节点接入';

  return (
    <div className="dashboard-page">
      <div className="dashboard-header">
        <div>
          <span className="bp-eyebrow">控制总览</span>
          <Title level={3} className="dashboard-title">
            易链管理端
          </Title>
          <Text type="secondary">
            实时查看用户、节点、站点网关与服务端路由状态。
          </Text>
        </div>
        <Space>
          <Button onClick={() => navigate('/peers')}>节点管理</Button>
          <Button icon={<BellOutlined />} onClick={() => navigate('/notifications')}>
            通知设置
          </Button>
          <Button type="primary" onClick={() => navigate('/connect')}>
            接入指南
          </Button>
        </Space>
      </div>

      <div className="dashboard-status-strip">
        <div className="dashboard-status-main">
          <span className={staleOfflineGateways.length > 0 ? 'status-dot warning' : 'status-dot ok'} />
          <div>
            <div className="dashboard-status-title">{headlineStatus}</div>
            <div className="dashboard-status-note">
              {peersPage ? `${onlineCount}/${peers.length} 个节点在线，在线率 ${healthScore}%` : '正在加载节点状态'}
            </div>
          </div>
        </div>
        <div className="dashboard-status-facts">
          <div>
            <span>站点网关</span>
            <strong>{gatewayCount}</strong>
          </div>
          <div>
            <span>路由网段</span>
            <strong>{routeCount}</strong>
          </div>
          <div>
            <span>24h 新增</span>
            <strong>{newTodayCount}</strong>
          </div>
        </div>
      </div>

      {topRisk && (
        <Alert
          className="dashboard-alert"
          type="warning"
          showIcon
          message={`站点网关离线：${topRisk.deviceName}（${topRisk.username}）已离线 ${now.diff(
            dayjs(topRisk.lastSeenAt),
            'minute'
          )} 分钟`}
          description={
            staleOfflineGateways.length > 1
              ? `还有 ${staleOfflineGateways.length - 1} 个站点网关也超过离线阈值。`
              : '该网关承载站点 LAN 网段，建议优先检查客户端网络、权限和服务端连通性。'
          }
          action={
            <Button size="small" onClick={() => navigate('/peers')}>
              查看节点
            </Button>
          }
        />
      )}

      <Row gutter={[16, 16]} className="dashboard-metrics">
        <Col xs={12} xl={6}>
          <MetricCard
            label="用户总数"
            value={usersPage?.total ?? '—'}
            note="已创建账号"
            icon={<TeamOutlined />}
            tone="blue"
          />
        </Col>
        <Col xs={12} xl={6}>
          <MetricCard
            label="在线节点"
            value={peersPage ? onlineCount : '—'}
            note={`${offlineCount} 个离线`}
            icon={<ApiOutlined />}
            tone="green"
          />
        </Col>
        <Col xs={12} xl={6}>
          <MetricCard
            label="站点网关"
            value={peersPage ? gatewayCount : '—'}
            note={`${routeCount} 条可路由网段`}
            icon={<ClusterOutlined />}
            tone="amber"
          />
        </Col>
        <Col xs={12} xl={6}>
          <MetricCard
            label="24 小时新增"
            value={peersPage ? newTodayCount : '—'}
            note="新注册节点"
            icon={<FieldTimeOutlined />}
          />
        </Col>
      </Row>

      <SectionHead eyebrow="拓扑" title="网关拓扑" />
      <GatewayTopology peers={peers} serverRoutes={system?.serverRoutes ?? []} />

      <Row gutter={[20, 20]}>
        <Col xs={24} xl={16}>
          <SectionHead eyebrow="健康" title="节点健康" />
          <Card className="dashboard-panel dashboard-health-panel" styles={{ body: { padding: 4 } }}>
            <PeerHealthTable peers={peers} />
          </Card>

          <SectionHead eyebrow="变更" title="最近变更记录" />
          <PeerEventsCard />
        </Col>
        <Col xs={24} xl={8}>
          <SectionHead
            eyebrow="系统"
            title="服务端状态"
            extra={
              system ? (
                <Button type="link" size="small" onClick={() => setEditRoutes(true)}>
                  编辑 LAN 网段
                </Button>
              ) : undefined
            }
          />
          <Card className="dashboard-panel" styles={{ body: { padding: sysLoading ? 20 : 14 } }}>
            {sysError && (
              <Alert
                type="error"
                showIcon
                message="无法加载系统信息"
                description={sysErr instanceof Error ? sysErr.message : '未知错误'}
              />
            )}
            {sysLoading && <Skeleton active paragraph={{ rows: 5 }} />}
            {system && (
              <div className="dashboard-system-grid">
                <div className="dashboard-system-row">
                  <span>版本</span>
                  <Text code>v{system.version}</Text>
                </div>
                <div className="dashboard-system-row">
                  <span>监听端口</span>
                  <Text code>{system.listenPort}</Text>
                </div>
                <div className="dashboard-system-row">
                  <span>Endpoint</span>
                  <Text code copyable={{ text: system.serverEndpoint }} className="dashboard-system-code">
                    {system.serverEndpoint}
                  </Text>
                </div>
                <div className="dashboard-system-row">
                  <span>虚拟子网</span>
                  <Text code>{system.vpnSubnet}</Text>
                </div>
                <div className="dashboard-system-row">
                  <span>服务端 LAN</span>
                  {(system.serverRoutes ?? []).length > 0 ? (
                    <Space size={[4, 4]} wrap className="dashboard-system-routes">
                      {system.serverRoutes!.map((route) => (
                        <Tag key={route} color="blue">
                          {route}
                        </Tag>
                      ))}
                    </Space>
                  ) : (
                    <Text type="secondary">未配置</Text>
                  )}
                </div>
                <div className="dashboard-system-row">
                  <span>启动时间</span>
                  <Text>{dayjs(system.startedAt).fromNow()}</Text>
                </div>
              </div>
            )}
          </Card>

          <Card className="dashboard-route-card">
            <Space direction="vertical" size={8}>
              <Space>
                <SafetyCertificateOutlined />
                <Text strong>路由覆盖</Text>
              </Space>
              <Text type="secondary">
                服务端 LAN 与站点网关共维护 {routeCount} 条网段。
              </Text>
              <Space size={[6, 6]} wrap>
                <Tag icon={<GlobalOutlined />} color="processing">
                  服务端 {(system?.serverRoutes ?? []).length}
                </Tag>
                <Tag color="blue">网关 {gatewayCount}</Tag>
                {staleOfflineGateways.length > 0 && (
                  <Tag icon={<ExclamationCircleOutlined />} color="warning">
                    网关告警 {staleOfflineGateways.length}
                  </Tag>
                )}
              </Space>
            </Space>
          </Card>

          <SectionHead eyebrow="活动" title="最近活动" />
          <Card className="dashboard-panel" styles={{ body: { padding: recentPeers.length === 0 ? 20 : 0 } }}>
            {recentPeers.length === 0 ? (
              <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="暂无活动节点" />
            ) : (
              <List
                className="dashboard-peer-list compact"
                dataSource={recentPeers.slice(0, 5)}
                renderItem={(peer) => (
                  <List.Item>
                    <div className="dashboard-peer-row">
                      <Space size="middle" className="dashboard-peer-main">
                        <NodeStatusDot status={peer.status} size="sm" />
                        <div>
                          <div className="dashboard-peer-name">
                            {peer.deviceName}
                            {(peer.routedSubnets ?? []).length > 0 && <Tag color="blue">网关</Tag>}
                          </div>
                          <Text type="secondary" className="dashboard-peer-meta">
                            {peer.username} · {peer.vpnIp}
                          </Text>
                        </div>
                      </Space>
                      <Tooltip title={peer.lastSeenAt ? dayjs(peer.lastSeenAt).format('YYYY-MM-DD HH:mm:ss') : '从未'}>
                        <Text type="secondary" className="dashboard-peer-time">{peerLastSeenLabel(peer)}</Text>
                      </Tooltip>
                    </div>
                  </List.Item>
                )}
              />
            )}
          </Card>
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
