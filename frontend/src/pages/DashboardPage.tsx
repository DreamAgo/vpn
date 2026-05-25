/**
 * Story 2.12 + 5.9：仪表盘（KPI 卡片 + 离线告警 + 最近节点 + 系统信息）。
 *
 * - 4 个 KPI：用户总数 / 在线节点 / 离线节点 / 今日新增（最近 24h 接入的 peer）。
 * - 离线超 10 分钟的节点 → Alert 警告 + "处理"按钮跳 /peers。
 * - 最近心跳的 10 个节点列表（NodeStatusDot + 设备名 + 虚拟 IP + 相对时间）。
 * - 保留系统信息卡片。
 * - 节点数据每 10s 轮询（refetchInterval）。
 */
import {
  Card,
  Col,
  Row,
  Statistic,
  Descriptions,
  Tag,
  Skeleton,
  Alert,
  Typography,
  List,
  Space,
} from 'antd';
import {
  ApiOutlined,
  ClusterOutlined,
  UserOutlined,
  ThunderboltOutlined,
} from '@ant-design/icons';
import { useQuery } from '@tanstack/react-query';
import { useNavigate } from 'react-router-dom';
import dayjs from 'dayjs';
import relativeTime from 'dayjs/plugin/relativeTime';
import 'dayjs/locale/zh-cn';

import { systemApi } from '@/services/auth';
import { usersApi } from '@/services/users';
import { peersApi } from '@/services/peers';
import { NodeStatusDot } from '@/components/NodeStatusDot';
import type { AdminPeerView } from '@/types/api';

dayjs.extend(relativeTime);
dayjs.locale('zh-cn');

const { Title, Text, Link } = Typography;

const OFFLINE_ALERT_MINUTES = 10;

export function DashboardPage() {
  const navigate = useNavigate();

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

  // 离线超 10 分钟的节点（按离线时长降序，最多展示前几条）。
  const now = dayjs();
  const staleOffline = peers
    .filter(
      (p) =>
        p.status === 'offline' &&
        p.lastSeenAt != null &&
        now.diff(dayjs(p.lastSeenAt), 'minute') >= OFFLINE_ALERT_MINUTES
    )
    .sort((a, b) => (a.lastSeenAt ?? 0) - (b.lastSeenAt ?? 0));

  // 最近心跳的 10 个节点（有心跳的优先，按 lastSeenAt 降序）。
  const recentPeers = [...peers]
    .filter((p) => p.lastSeenAt != null)
    .sort((a, b) => (b.lastSeenAt ?? 0) - (a.lastSeenAt ?? 0))
    .slice(0, 10);

  return (
    <div style={{ padding: 24 }}>
      <Title level={4} style={{ marginBottom: 16 }}>
        仪表盘
      </Title>

      {/* 离线告警 */}
      {staleOffline.length > 0 && (
        <Space direction="vertical" style={{ width: '100%', marginBottom: 16 }} size="small">
          {staleOffline.slice(0, 5).map((p) => (
            <Alert
              key={p.id}
              type="warning"
              showIcon
              message={`节点 ${p.deviceName}（${p.username}）已离线 ${now.diff(
                dayjs(p.lastSeenAt),
                'minute'
              )} 分钟`}
              action={
                <a onClick={() => navigate('/peers')} style={{ whiteSpace: 'nowrap' }}>
                  处理
                </a>
              }
            />
          ))}
        </Space>
      )}

      {/* KPI 卡片 */}
      <Row gutter={[16, 16]} style={{ marginBottom: 16 }}>
        <Col xs={24} sm={12} lg={6}>
          <Card>
            <Statistic
              title="用户总数"
              value={usersPage?.total ?? '—'}
              prefix={<UserOutlined />}
            />
          </Card>
        </Col>
        <Col xs={24} sm={12} lg={6}>
          <Card>
            <Statistic
              title="在线节点"
              value={peersPage ? onlineCount : '—'}
              prefix={<ApiOutlined />}
              valueStyle={{ color: '#52C41A' }}
            />
          </Card>
        </Col>
        <Col xs={24} sm={12} lg={6}>
          <Card>
            <Statistic
              title="离线节点"
              value={peersPage ? offlineCount : '—'}
              prefix={<ClusterOutlined />}
              valueStyle={{ color: '#8c8c8c' }}
            />
          </Card>
        </Col>
        <Col xs={24} sm={12} lg={6}>
          <Card>
            <Statistic
              title="今日新增"
              value={peersPage ? newTodayCount : '—'}
              prefix={<ThunderboltOutlined />}
            />
          </Card>
        </Col>
      </Row>

      <Row gutter={[16, 16]}>
        {/* 最近节点 */}
        <Col xs={24} lg={12}>
          <Card
            title="最近活动节点"
            extra={<Link onClick={() => navigate('/peers')}>查看全部 →</Link>}
          >
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
                      <Text type="secondary">
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
          <Card title="系统信息">
            {sysError && (
              <Alert
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
                  <Tag color="blue">v{system.version}</Tag>
                </Descriptions.Item>
                <Descriptions.Item label="监听端口">{system.listenPort}</Descriptions.Item>
                <Descriptions.Item label="VPN 子网">{system.vpnSubnet}</Descriptions.Item>
                <Descriptions.Item label="服务端 Endpoint">
                  {system.serverEndpoint}
                </Descriptions.Item>
                <Descriptions.Item label="服务端公钥">
                  <Typography.Text
                    code
                    copyable={{ text: system.serverPublicKey }}
                    style={{ wordBreak: 'break-all' }}
                  >
                    {system.serverPublicKey}
                  </Typography.Text>
                </Descriptions.Item>
                <Descriptions.Item label="启动时间">
                  {dayjs(system.startedAt).format('YYYY-MM-DD HH:mm:ss')}
                </Descriptions.Item>
              </Descriptions>
            )}
          </Card>
        </Col>
      </Row>
    </div>
  );
}
