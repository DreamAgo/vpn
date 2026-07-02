/**
 * 节点健康监控（仪表盘）：
 * - 健康表格：在线时长 / 最近心跳 / 延迟 / 丢包 / 客户端版本；
 * - 每个节点可打开"变更记录"抽屉（OS / IP / Endpoint / 设备名 / 版本变化历史）；
 * - 全局"最近变更"卡片（跨节点最近 20 条）。
 */
import { useState } from 'react';
import { Card, Table, Tag, Space, Drawer, List, Button, Typography, Empty } from 'antd';
import { HistoryOutlined } from '@ant-design/icons';
import { useQuery } from '@tanstack/react-query';
import dayjs from 'dayjs';

import { peersApi } from '@/services/peers';
import { NodeStatusDot } from '@/components/NodeStatusDot';
import type { AdminPeerView, PeerEventView } from '@/types/api';

const { Text } = Typography;

/** 变更字段 → 中文标签。 */
const FIELD_LABELS: Record<string, string> = {
  os_info: 'OS',
  endpoint: 'Endpoint',
  vpn_ip: 'VPN IP',
  device_name: '设备名',
  client_version: '客户端版本',
};

function fieldLabel(field: string): string {
  return FIELD_LABELS[field] ?? field;
}

/** 毫秒时长 → "3 天 2 小时" / "5 小时 12 分" / "8 分钟" / "45 秒"。 */
function formatDuration(ms: number): string {
  const sec = Math.floor(ms / 1000);
  if (sec < 60) return `${sec} 秒`;
  const min = Math.floor(sec / 60);
  if (min < 60) return `${min} 分钟`;
  const hour = Math.floor(min / 60);
  if (hour < 24) return `${hour} 小时 ${min % 60} 分`;
  return `${Math.floor(hour / 24)} 天 ${hour % 24} 小时`;
}

/** 延迟着色：<100ms 良好，<300ms 一般，其余偏高。 */
function RttCell({ rttMs }: { rttMs: number | null }) {
  if (rttMs == null) return <Text type="secondary">—</Text>;
  const color = rttMs < 100 ? '#16a34a' : rttMs < 300 ? '#d97706' : '#dc2626';
  return (
    <span style={{ fontFamily: 'var(--font-mono)', color }}>
      {rttMs} ms
    </span>
  );
}

/** 丢包着色：0 良好，<10% 一般，其余偏高。 */
function LossCell({ lossPct }: { lossPct: number | null }) {
  if (lossPct == null) return <Text type="secondary">—</Text>;
  const color = lossPct <= 0 ? '#16a34a' : lossPct < 10 ? '#d97706' : '#dc2626';
  return (
    <span style={{ fontFamily: 'var(--font-mono)', color }}>
      {lossPct.toFixed(1)}%
    </span>
  );
}

/** 变更记录列表（抽屉与全局卡片共用）。 */
function EventList({ events, showDevice }: { events: PeerEventView[]; showDevice: boolean }) {
  if (events.length === 0) {
    return <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="暂无变更记录" />;
  }
  return (
    <List
      size="small"
      dataSource={events}
      renderItem={(e) => (
        <List.Item>
          <Space direction="vertical" size={2} style={{ width: '100%' }}>
            <Space size="small" wrap>
              <Tag color="geekblue">{fieldLabel(e.field)}</Tag>
              {showDevice && (
                <Text strong>
                  {e.deviceName ?? '（已删除节点）'}
                  {e.username ? ` · ${e.username}` : ''}
                </Text>
              )}
              <Text type="secondary" style={{ fontSize: 12 }} title={dayjs(e.createdAt).format('YYYY-MM-DD HH:mm:ss')}>
                {dayjs(e.createdAt).fromNow()}
              </Text>
            </Space>
            <span style={{ fontFamily: 'var(--font-mono)', fontSize: 12, wordBreak: 'break-all' }}>
              {e.oldValue ? (
                <>
                  <Text delete type="secondary">
                    {e.oldValue}
                  </Text>
                  {' → '}
                </>
              ) : null}
              <Text>{e.newValue ?? '（清空）'}</Text>
            </span>
          </Space>
        </List.Item>
      )}
    />
  );
}

/** 全局最近变更卡片（跨节点最近 20 条，30s 自动刷新）。 */
export function PeerEventsCard() {
  const { data: events } = useQuery({
    queryKey: ['peer-events-recent'],
    queryFn: () => peersApi.listPeerEvents({ limit: 20 }),
    refetchInterval: 30_000,
  });
  return (
    <Card styles={{ body: { padding: (events ?? []).length === 0 ? 20 : 4, maxHeight: 420, overflow: 'auto' } }}>
      <EventList events={events ?? []} showDevice />
    </Card>
  );
}

/** 节点健康表格 + 单节点变更记录抽屉。 */
export function PeerHealthTable({ peers }: { peers: AdminPeerView[] }) {
  const [eventPeer, setEventPeer] = useState<AdminPeerView | null>(null);
  const now = dayjs();

  const { data: peerEvents, isFetching } = useQuery({
    queryKey: ['peer-events', eventPeer?.id],
    queryFn: () => peersApi.listPeerEvents({ peerId: eventPeer!.id, limit: 100 }),
    enabled: eventPeer !== null,
  });

  return (
    <>
      <Table<AdminPeerView>
        rowKey="id"
        size="small"
        dataSource={peers}
        pagination={peers.length > 10 ? { pageSize: 10, showSizeChanger: false } : false}
        columns={[
          {
            title: '状态',
            dataIndex: 'status',
            width: 70,
            render: (_, p) => <NodeStatusDot status={p.status} size="sm" />,
          },
          {
            title: '设备 / 用户',
            dataIndex: 'deviceName',
            ellipsis: true,
            render: (_, p) => (
              <Space size={6}>
                <Text strong>{p.deviceName}</Text>
                <Text type="secondary" style={{ fontSize: 12 }}>
                  {p.username}
                </Text>
              </Space>
            ),
          },
          {
            title: 'VPN IP',
            dataIndex: 'vpnIp',
            width: 110,
            render: (v) => <Text code>{v}</Text>,
          },
          {
            title: '在线时长',
            dataIndex: 'onlineSince',
            width: 120,
            render: (_, p) =>
              p.status === 'online' && p.onlineSince ? (
                <span style={{ fontFamily: 'var(--font-mono)' }}>
                  {formatDuration(now.valueOf() - p.onlineSince)}
                </span>
              ) : (
                <Text type="secondary">—</Text>
              ),
          },
          {
            title: '最近心跳',
            dataIndex: 'lastSeenAt',
            width: 110,
            render: (_, p) =>
              p.lastSeenAt ? (
                <span title={dayjs(p.lastSeenAt).format('YYYY-MM-DD HH:mm:ss')}>
                  {dayjs(p.lastSeenAt).fromNow()}
                </span>
              ) : (
                <Text type="secondary">从未</Text>
              ),
          },
          {
            title: '延迟',
            dataIndex: 'rttMs',
            width: 90,
            render: (_, p) => <RttCell rttMs={p.rttMs} />,
          },
          {
            title: '丢包',
            dataIndex: 'lossPct',
            width: 80,
            render: (_, p) => <LossCell lossPct={p.lossPct} />,
          },
          {
            title: '版本',
            dataIndex: 'clientVersion',
            width: 90,
            render: (v) => (v ? <Tag>{v}</Tag> : <Text type="secondary">—</Text>),
          },
          {
            title: '',
            key: 'events',
            width: 110,
            render: (_, p) => (
              <Button
                type="link"
                size="small"
                icon={<HistoryOutlined />}
                onClick={() => setEventPeer(p)}
              >
                变更记录
              </Button>
            ),
          },
        ]}
        locale={{ emptyText: <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="暂无节点" /> }}
      />

      <Drawer
        open={eventPeer !== null}
        onClose={() => setEventPeer(null)}
        width={440}
        title={eventPeer ? `变更记录 · ${eventPeer.deviceName}（${eventPeer.username}）` : '变更记录'}
        loading={isFetching}
      >
        {eventPeer && (
          <Space direction="vertical" size={2} style={{ marginBottom: 12 }}>
            <Text type="secondary" style={{ fontSize: 12 }}>
              OS：{eventPeer.osInfo ?? '未知'} · Endpoint：{eventPeer.endpoint ?? '未知'}
            </Text>
            <Text type="secondary" style={{ fontSize: 12 }}>
              近 30 天内的 OS / IP / Endpoint / 设备名 / 版本变化
            </Text>
          </Space>
        )}
        <EventList events={peerEvents ?? []} showDevice={false} />
      </Drawer>
    </>
  );
}
