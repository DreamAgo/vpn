/**
 * 用户组管理页。
 *
 * - 列表展示组名、可路由网段(Tag)、成员数、创建时间。
 * - 新建 / 编辑(组名 + 网段)/ 删除(删除后成员自动解组)。
 * - 组的可路由网段决定成员的 VPN allowed_routes(访问控制)。
 */
import { useState } from 'react';
import {
  Button,
  Tag,
  Space,
  Popconfirm,
  App,
  Typography,
  Table,
  Tooltip,
} from 'antd';
import { PlusOutlined } from '@ant-design/icons';
import { useQuery } from '@tanstack/react-query';
import dayjs from 'dayjs';

import { groupsApi } from '@/services/groups';
import { ApiError } from '@/services/http';
import { ErrorCodes } from '@/types/api';
import type { UserGroupDto } from '@/types/api';
import { EditGroupModal } from '@/components/EditGroupModal';

const { Title, Text } = Typography;

function describeError(err: unknown, fallback: string): string {
  if (err instanceof ApiError) {
    switch (err.code) {
      case ErrorCodes.DuplicateResource:
        return '用户组名称已存在';
      case ErrorCodes.NoAccess:
      case ErrorCodes.RequireAdmin:
        return '无权限执行该操作';
      default:
        return err.message || fallback;
    }
  }
  return fallback;
}

export function GroupsPage() {
  const { message } = App.useApp();
  const [editing, setEditing] = useState<UserGroupDto | null>(null);
  const [modalOpen, setModalOpen] = useState(false);

  const { data: groups, isLoading, refetch } = useQuery({
    queryKey: ['groups'],
    queryFn: groupsApi.listGroups,
  });

  const openCreate = () => {
    setEditing(null);
    setModalOpen(true);
  };
  const openEdit = (g: UserGroupDto) => {
    setEditing(g);
    setModalOpen(true);
  };

  const handleDelete = async (g: UserGroupDto) => {
    try {
      await groupsApi.deleteGroup(g.id);
      message.success('已删除用户组（成员已自动解组）');
      refetch();
    } catch (err) {
      message.error(describeError(err, '删除失败'));
    }
  };

  const columns = [
    {
      title: '组名',
      dataIndex: 'name',
      key: 'name',
      render: (name: string) => <Text strong>{name}</Text>,
    },
    {
      title: '可路由网段',
      dataIndex: 'routes',
      key: 'routes',
      render: (routes: string[]) =>
        routes.length === 0 ? (
          <Text type="secondary">—（仅 VPN 子网 + 站点网关）</Text>
        ) : (
          <Space size={[0, 4]} wrap>
            {routes.map((r) => (
              <Tag key={r} color="blue">
                {r}
              </Tag>
            ))}
          </Space>
        ),
    },
    {
      title: '成员数',
      dataIndex: 'memberCount',
      key: 'memberCount',
      width: 90,
      render: (n: number) => <Tag>{n}</Tag>,
    },
    {
      title: '创建时间',
      dataIndex: 'createdAt',
      key: 'createdAt',
      width: 170,
      render: (t: number) => (
        <Text type="secondary">{dayjs(t).format('YYYY-MM-DD HH:mm')}</Text>
      ),
    },
    {
      title: '操作',
      key: 'action',
      width: 140,
      render: (_: unknown, record: UserGroupDto) => (
        <Space size="small">
          <Button type="link" size="small" onClick={() => openEdit(record)}>
            编辑
          </Button>
          <Popconfirm
            title="删除用户组"
            description={
              record.memberCount > 0
                ? `该组有 ${record.memberCount} 名成员，删除后他们将解组并回退到全局默认网段。`
                : '确定删除该用户组？'
            }
            okText="删除"
            okButtonProps={{ danger: true }}
            cancelText="取消"
            onConfirm={() => handleDelete(record)}
          >
            <Button type="link" size="small" danger>
              删除
            </Button>
          </Popconfirm>
        </Space>
      ),
    },
  ];

  return (
    <div>
      <Space
        align="center"
        style={{ width: '100%', justifyContent: 'space-between', marginBottom: 16 }}
      >
        <Title level={4} style={{ margin: 0 }}>
          用户组
          <Tooltip title="组的可路由网段决定其成员经 VPN 可访问的网段（访问控制）。未分组用户回退到全局默认网段。">
            <Text type="secondary" style={{ fontSize: 13, marginLeft: 12, fontWeight: 400 }}>
              按组授权可访问网段
            </Text>
          </Tooltip>
        </Title>
        <Button type="primary" icon={<PlusOutlined />} onClick={openCreate}>
          新建用户组
        </Button>
      </Space>

      <Table<UserGroupDto>
        rowKey="id"
        loading={isLoading}
        columns={columns}
        dataSource={groups ?? []}
        pagination={false}
      />

      <EditGroupModal
        open={modalOpen}
        group={editing}
        onClose={() => setModalOpen(false)}
        onSaved={() => refetch()}
      />
    </div>
  );
}
