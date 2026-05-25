/**
 * Story 5.7：节点管理页（ProTable）。
 *
 * - ProTable request 模式调 GET /admin/peers（分页 + 搜索 + 状态筛选）。
 * - 搜索框防抖 300ms（匹配 username / device_name）+ 状态筛选下拉。
 * - 每 10s 轮询（ProTable polling）。
 * - 操作列：详情（Drawer 展示完整字段）+ 强制下线（红色 Popconfirm → DELETE /admin/peers/:id）。
 * - 空态：无任何节点 → peers-empty；搜索/筛选无结果 → search-empty。
 */
import { useCallback, useMemo, useRef, useState } from 'react';
import {
  Button,
  Space,
  Input,
  Select,
  Popconfirm,
  App,
  Typography,
  Drawer,
  Descriptions,
} from 'antd';
import { ProTable, type ActionType, type ProColumns } from '@ant-design/pro-components';
import dayjs from 'dayjs';
import relativeTime from 'dayjs/plugin/relativeTime';
import 'dayjs/locale/zh-cn';
import { useNavigate } from 'react-router-dom';

import { peersApi } from '@/services/peers';
import { ApiError } from '@/services/http';
import { ErrorCodes } from '@/types/api';
import type { AdminPeerView } from '@/types/api';
import { NodeStatusDot } from '@/components/NodeStatusDot';
import { EmptyStateWithAction } from '@/components/EmptyStateWithAction';

dayjs.extend(relativeTime);
dayjs.locale('zh-cn');

const { Title, Text } = Typography;

function describeError(err: unknown, fallback: string): string {
  if (err instanceof ApiError) {
    switch (err.code) {
      case ErrorCodes.PeerNotFound:
        return '节点不存在或已被移除';
      case ErrorCodes.NoAccess:
      case ErrorCodes.RequireAdmin:
        return '无权限执行该操作';
      default:
        return err.message || fallback;
    }
  }
  return fallback;
}

export function PeersPage() {
  const { message } = App.useApp();
  const navigate = useNavigate();
  const actionRef = useRef<ActionType>(null);

  const [search, setSearch] = useState('');
  const [statusFilter, setStatusFilter] = useState<string | undefined>(undefined);
  const [searchInput, setSearchInput] = useState('');
  const debounceTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const [detail, setDetail] = useState<AdminPeerView | null>(null);

  const reload = useCallback(() => actionRef.current?.reload(), []);

  const handleSearchChange = (value: string) => {
    setSearchInput(value);
    if (debounceTimer.current) clearTimeout(debounceTimer.current);
    debounceTimer.current = setTimeout(() => {
      setSearch(value.trim());
      reload();
    }, 300);
  };

  const clearSearch = () => {
    setSearchInput('');
    setSearch('');
    setStatusFilter(undefined);
    reload();
  };

  const handleForceRemove = async (peer: AdminPeerView) => {
    try {
      await peersApi.forceRemovePeer(peer.id);
      message.success('已强制下线节点');
      reload();
    } catch (err) {
      message.error(describeError(err, '强制下线失败'));
    }
  };

  const hasFilter = search.length > 0 || statusFilter !== undefined;

  const columns = useMemo<ProColumns<AdminPeerView>[]>(
    () => [
      {
        title: '状态',
        dataIndex: 'status',
        width: 110,
        render: (_, record) => (
          <NodeStatusDot status={record.status} lastSeen={record.lastSeenAt} />
        ),
      },
      {
        title: '设备名',
        dataIndex: 'deviceName',
        ellipsis: true,
      },
      {
        title: '所属用户',
        dataIndex: 'username',
        ellipsis: true,
      },
      {
        title: '虚拟 IP',
        dataIndex: 'vpnIp',
        width: 140,
        render: (_, record) => <Text code>{record.vpnIp}</Text>,
      },
      {
        title: '最后心跳',
        dataIndex: 'lastSeenAt',
        width: 140,
        render: (_, record) =>
          record.lastSeenAt ? (
            <span title={dayjs(record.lastSeenAt).format('YYYY-MM-DD HH:mm:ss')}>
              {dayjs(record.lastSeenAt).fromNow()}
            </span>
          ) : (
            <Text type="secondary">从未</Text>
          ),
      },
      {
        title: '公网 endpoint',
        dataIndex: 'endpoint',
        ellipsis: true,
        render: (_, record) =>
          record.endpoint ? record.endpoint : <Text type="secondary">—</Text>,
      },
      {
        title: '操作',
        key: 'action',
        width: 150,
        render: (_, record) => (
          <Space size="small">
            <Button type="link" size="small" onClick={() => setDetail(record)}>
              详情
            </Button>
            <Popconfirm
              title="强制下线"
              description="将立即撤销该节点的接入，确定强制下线？"
              okText="强制下线"
              okButtonProps={{ danger: true }}
              cancelText="取消"
              onConfirm={() => handleForceRemove(record)}
            >
              <Button type="link" size="small" danger>
                强制下线
              </Button>
            </Popconfirm>
          </Space>
        ),
      },
    ],
    // eslint-disable-next-line react-hooks/exhaustive-deps
    []
  );

  return (
    <div style={{ padding: 24 }}>
      <Title level={4} style={{ marginBottom: 16 }}>
        节点管理
      </Title>

      <ProTable<AdminPeerView>
        actionRef={actionRef}
        rowKey="id"
        columns={columns}
        search={false}
        polling={10_000}
        options={{ reload: true, density: false, setting: false }}
        pagination={{ defaultPageSize: 10, showSizeChanger: true }}
        toolbar={{
          search: (
            <Space>
              <Input.Search
                allowClear
                placeholder="搜索用户名 / 设备名"
                value={searchInput}
                onChange={(e) => handleSearchChange(e.target.value)}
                style={{ width: 240 }}
              />
              <Select
                allowClear
                placeholder="状态"
                value={statusFilter}
                style={{ width: 130 }}
                onChange={(v) => {
                  setStatusFilter(v);
                  reload();
                }}
                options={[
                  { value: 'online', label: '在线' },
                  { value: 'offline', label: '离线' },
                  { value: 'deleted', label: '已删除' },
                  { value: 'force_removed', label: '已下线' },
                ]}
              />
            </Space>
          ),
        }}
        request={async (params) => {
          try {
            const page = await peersApi.listAdminPeers({
              page: params.current,
              pageSize: params.pageSize,
              search: search || undefined,
              status: statusFilter,
            });
            return {
              data: page.items,
              total: page.total,
              success: true,
            };
          } catch (err) {
            message.error(describeError(err, '加载节点列表失败'));
            return { data: [], total: 0, success: false };
          }
        }}
        locale={{
          emptyText: hasFilter ? (
            <EmptyStateWithAction variant="search-empty" onAction={clearSearch} />
          ) : (
            <EmptyStateWithAction
              variant="peers-empty"
              onAction={() => navigate('/connect')}
            />
          ),
        }}
      />

      <Drawer
        title="节点详情"
        width={520}
        open={detail !== null}
        onClose={() => setDetail(null)}
      >
        {detail && (
          <Descriptions column={1} bordered size="small">
            <Descriptions.Item label="状态">
              <NodeStatusDot status={detail.status} lastSeen={detail.lastSeenAt} />
            </Descriptions.Item>
            <Descriptions.Item label="设备名">{detail.deviceName}</Descriptions.Item>
            <Descriptions.Item label="所属用户">{detail.username}</Descriptions.Item>
            <Descriptions.Item label="邮箱">{detail.email}</Descriptions.Item>
            <Descriptions.Item label="虚拟 IP">
              <Text code>{detail.vpnIp}</Text>
            </Descriptions.Item>
            <Descriptions.Item label="公网 endpoint">
              {detail.endpoint ?? '—'}
            </Descriptions.Item>
            <Descriptions.Item label="操作系统">{detail.osInfo ?? '—'}</Descriptions.Item>
            <Descriptions.Item label="WireGuard 公钥">
              <Text code copyable={{ text: detail.wgPublicKey }} style={{ wordBreak: 'break-all' }}>
                {detail.wgPublicKey}
              </Text>
            </Descriptions.Item>
            <Descriptions.Item label="最后心跳">
              {detail.lastSeenAt
                ? `${dayjs(detail.lastSeenAt).format('YYYY-MM-DD HH:mm:ss')}（${dayjs(
                    detail.lastSeenAt
                  ).fromNow()}）`
                : '从未'}
            </Descriptions.Item>
            <Descriptions.Item label="创建时间">
              {dayjs(detail.createdAt).format('YYYY-MM-DD HH:mm:ss')}
            </Descriptions.Item>
          </Descriptions>
        )}
      </Drawer>
    </div>
  );
}
