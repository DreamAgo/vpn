import { useMemo, useState } from 'react';
import { Badge, Button, Empty, List, Popover, Space, Tag, Typography } from 'antd';
import { BellOutlined, FileTextOutlined, NodeIndexOutlined } from '@ant-design/icons';
import { useQuery } from '@tanstack/react-query';
import { useNavigate } from 'react-router-dom';
import dayjs from 'dayjs';

import { auditApi } from '@/services/audit';
import { peersApi } from '@/services/peers';
import type { AuditLogDto, PeerEventView } from '@/types/api';

const { Text } = Typography;
const LAST_READ_KEY = 'yilian.notifications.lastReadAt';

type NotificationItem = {
  id: string;
  type: 'audit' | 'peer';
  title: string;
  detail: string;
  createdAt: number;
  tone: string;
};

function readLastReadAt(): number {
  const raw = window.localStorage.getItem(LAST_READ_KEY);
  const n = raw ? Number(raw) : 0;
  return Number.isFinite(n) ? n : 0;
}

function actionTone(action: string): string {
  const a = action.toLowerCase();
  if (a.includes('delete') || a.includes('remove') || a.includes('force')) return 'red';
  if (a.includes('login') || a.includes('logout') || a.includes('auth')) return 'blue';
  if (a.includes('create') || a.includes('register')) return 'green';
  if (a.includes('update') || a.includes('change') || a.includes('reset')) return 'orange';
  return 'default';
}

function auditToItem(item: AuditLogDto): NotificationItem {
  const failed = item.statusCode != null && item.statusCode >= 400;
  return {
    id: `audit:${item.id}`,
    type: 'audit',
    title: failed ? `操作失败：${item.action}` : item.action,
    detail: `${item.username ?? '系统'} · ${item.resource}`,
    createdAt: item.createdAt,
    tone: failed ? 'red' : actionTone(item.action),
  };
}

function peerToItem(item: PeerEventView): NotificationItem {
  return {
    id: `peer:${item.id}`,
    type: 'peer',
    title: `节点变更：${item.field}`,
    detail: `${item.deviceName ?? '已删除节点'}${item.username ? ` · ${item.username}` : ''}`,
    createdAt: item.createdAt,
    tone: 'purple',
  };
}

export function EventNotifications() {
  const navigate = useNavigate();
  const [open, setOpen] = useState(false);
  const [lastReadAt, setLastReadAt] = useState(readLastReadAt);
  const from = useMemo(() => dayjs().subtract(24, 'hour').valueOf(), []);

  const auditQuery = useQuery({
    queryKey: ['event-notifications', 'audit', from],
    queryFn: () => auditApi.listAuditLogs({ page: 1, pageSize: 8, from, to: Date.now() }),
    refetchInterval: 20_000,
  });

  const peerQuery = useQuery({
    queryKey: ['event-notifications', 'peer-events'],
    queryFn: () => peersApi.listPeerEvents({ limit: 8 }),
    refetchInterval: 20_000,
  });

  const items = useMemo(() => {
    const auditItems = (auditQuery.data?.items ?? []).map(auditToItem);
    const peerItems = (peerQuery.data ?? []).map(peerToItem);
    return [...auditItems, ...peerItems]
      .sort((a, b) => b.createdAt - a.createdAt)
      .slice(0, 12);
  }, [auditQuery.data, peerQuery.data]);

  const unread = items.filter((item) => item.createdAt > lastReadAt).length;

  const markRead = () => {
    const now = Date.now();
    window.localStorage.setItem(LAST_READ_KEY, String(now));
    setLastReadAt(now);
  };

  const content = (
    <div className="event-popover">
      <div className="event-popover-head">
        <Text strong>事件通知</Text>
        <Button type="link" size="small" onClick={markRead}>
          全部已读
        </Button>
      </div>
      {items.length === 0 ? (
        <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="暂无事件" />
      ) : (
        <List
          size="small"
          dataSource={items}
          renderItem={(item) => (
            <List.Item
              className={item.createdAt > lastReadAt ? 'event-item unread' : 'event-item'}
              onClick={() => {
                setOpen(false);
                navigate(item.type === 'audit' ? '/audit-logs' : '/peers');
              }}
            >
              <Space align="start" size={10}>
                {item.type === 'audit' ? <FileTextOutlined /> : <NodeIndexOutlined />}
                <div className="event-item-body">
                  <Space size={6} wrap>
                    <Tag color={item.tone}>{item.type === 'audit' ? '审计' : '节点'}</Tag>
                    <Text strong>{item.title}</Text>
                  </Space>
                  <div className="event-detail">{item.detail}</div>
                  <div className="event-time">{dayjs(item.createdAt).fromNow()}</div>
                </div>
              </Space>
            </List.Item>
          )}
        />
      )}
    </div>
  );

  return (
    <Popover
      trigger="click"
      placement="bottomRight"
      open={open}
      onOpenChange={(next) => {
        setOpen(next);
        if (next) markRead();
      }}
      content={content}
    >
      <Badge count={unread} size="small" offset={[-2, 4]}>
        <Button type="text" icon={<BellOutlined />} style={{ color: 'var(--ink-soft)' }} />
      </Badge>
    </Popover>
  );
}
