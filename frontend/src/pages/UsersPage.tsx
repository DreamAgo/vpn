/**
 * Story 3.6：用户管理页（ProTable）。
 *
 * - ProTable request 模式调 GET /admin/users（分页 + 搜索 + 状态筛选）。
 * - 搜索框防抖 300ms；状态下拉（全部/正常/已禁用）。
 * - 操作列：重置密码 / 启用·禁用 / 删除（均带确认，成功后刷新 + message）。
 * - 空态：无任何用户 → users-empty；搜索无结果 → search-empty。
 */
import { useCallback, useMemo, useRef, useState } from 'react';
import {
  Button,
  Tag,
  Space,
  Dropdown,
  Input,
  Select,
  Popconfirm,
  App,
  Typography,
} from 'antd';
import { ProTable, type ActionType, type ProColumns } from '@ant-design/pro-components';
import { MoreOutlined, PlusOutlined } from '@ant-design/icons';
import dayjs from 'dayjs';
import relativeTime from 'dayjs/plugin/relativeTime';
import 'dayjs/locale/zh-cn';

import { usersApi } from '@/services/users';
import { ApiError } from '@/services/http';
import { ErrorCodes } from '@/types/api';
import type { UserDto } from '@/types/api';
import { CreateUserModal } from '@/components/CreateUserModal';
import { ResetPasswordModal } from '@/components/ResetPasswordModal';
import { EmptyStateWithAction } from '@/components/EmptyStateWithAction';

dayjs.extend(relativeTime);
dayjs.locale('zh-cn');

const { Title } = Typography;

/** 将 ApiError 映射为中文提示。 */
function describeError(err: unknown, fallback: string): string {
  if (err instanceof ApiError) {
    switch (err.code) {
      case ErrorCodes.UserNotFound:
        return '用户不存在或已被删除';
      case ErrorCodes.DuplicateResource:
        return '用户名/邮箱已存在';
      case ErrorCodes.NoAccess:
      case ErrorCodes.RequireAdmin:
        return '无权限执行该操作';
      default:
        return err.message || fallback;
    }
  }
  return fallback;
}

export function UsersPage() {
  const { message } = App.useApp();
  const actionRef = useRef<ActionType>(null);

  const [search, setSearch] = useState('');
  const [statusFilter, setStatusFilter] = useState<string | undefined>(undefined);
  const [searchInput, setSearchInput] = useState('');
  const debounceTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const [createOpen, setCreateOpen] = useState(false);
  const [resetPwd, setResetPwd] = useState<string | null>(null);

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

  const handleResetPassword = async (user: UserDto) => {
    try {
      const res = await usersApi.resetPassword(user.id);
      setResetPwd(res.newPassword);
    } catch (err) {
      message.error(describeError(err, '重置密码失败'));
    }
  };

  const handleToggleStatus = async (user: UserDto) => {
    const next = user.status === 'active' ? 'disabled' : 'active';
    try {
      await usersApi.updateUser(user.id, { status: next });
      message.success(next === 'active' ? '已启用用户' : '已禁用用户');
      reload();
    } catch (err) {
      message.error(describeError(err, '更新状态失败'));
    }
  };

  const handleDelete = async (user: UserDto) => {
    try {
      await usersApi.deleteUser(user.id);
      message.success('已删除用户');
      reload();
    } catch (err) {
      message.error(describeError(err, '删除失败'));
    }
  };

  // 是否处于"有筛选条件"状态（决定空态变体）。
  const hasFilter = search.length > 0 || statusFilter !== undefined;

  const columns = useMemo<ProColumns<UserDto>[]>(
    () => [
      {
        title: '用户名',
        dataIndex: 'username',
        ellipsis: true,
      },
      {
        title: '邮箱',
        dataIndex: 'email',
        ellipsis: true,
      },
      {
        title: '状态',
        dataIndex: 'status',
        width: 100,
        render: (_, record) =>
          record.status === 'active' ? (
            <Tag color="success">正常</Tag>
          ) : (
            <Tag color="default">已禁用</Tag>
          ),
      },
      {
        title: '最后登录',
        dataIndex: 'lastLoginAt',
        width: 140,
        render: (_, record) =>
          record.lastLoginAt ? (
            <span title={dayjs(record.lastLoginAt).format('YYYY-MM-DD HH:mm:ss')}>
              {dayjs(record.lastLoginAt).fromNow()}
            </span>
          ) : (
            <Typography.Text type="secondary">从未登录</Typography.Text>
          ),
      },
      {
        title: '操作',
        key: 'action',
        width: 160,
        render: (_, record) => (
          <Space size="small">
            <Button
              type="link"
              size="small"
              onClick={() =>
                message.info(
                  `${record.username}（${record.email}）· 角色：${record.role} · 创建于 ${dayjs(
                    record.createdAt
                  ).format('YYYY-MM-DD')}`
                )
              }
            >
              详情
            </Button>
            <Dropdown
              trigger={['click']}
              menu={{
                items: [
                  {
                    key: 'reset',
                    label: (
                      <Popconfirm
                        title="重置密码"
                        description="将为该用户生成新的随机密码，旧密码立即失效。"
                        okText="重置"
                        cancelText="取消"
                        onConfirm={() => handleResetPassword(record)}
                      >
                        <span onClick={(e) => e.stopPropagation()}>重置密码</span>
                      </Popconfirm>
                    ),
                  },
                  {
                    key: 'toggle',
                    label: (
                      <span onClick={() => handleToggleStatus(record)}>
                        {record.status === 'active' ? '禁用' : '启用'}
                      </span>
                    ),
                  },
                  { type: 'divider' },
                  {
                    key: 'delete',
                    danger: true,
                    label: (
                      <Popconfirm
                        title="删除用户"
                        description="删除后不可恢复，确定删除该用户？"
                        okText="删除"
                        okButtonProps={{ danger: true }}
                        cancelText="取消"
                        onConfirm={() => handleDelete(record)}
                      >
                        <span onClick={(e) => e.stopPropagation()}>删除</span>
                      </Popconfirm>
                    ),
                  },
                ],
              }}
            >
              <Button type="text" size="small" icon={<MoreOutlined />} />
            </Dropdown>
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
        用户管理
      </Title>

      <ProTable<UserDto>
        actionRef={actionRef}
        rowKey="id"
        columns={columns}
        search={false}
        options={{ reload: true, density: false, setting: false }}
        pagination={{ defaultPageSize: 10, showSizeChanger: true }}
        toolbar={{
          search: (
            <Space>
              <Input.Search
                allowClear
                placeholder="搜索用户名 / 邮箱"
                value={searchInput}
                onChange={(e) => handleSearchChange(e.target.value)}
                style={{ width: 240 }}
              />
              <Select
                allowClear
                placeholder="状态"
                value={statusFilter}
                style={{ width: 120 }}
                onChange={(v) => {
                  setStatusFilter(v);
                  reload();
                }}
                options={[
                  { value: 'active', label: '正常' },
                  { value: 'disabled', label: '已禁用' },
                ]}
              />
            </Space>
          ),
          actions: [
            <Button
              key="create"
              type="primary"
              icon={<PlusOutlined />}
              onClick={() => setCreateOpen(true)}
            >
              新建用户
            </Button>,
          ],
        }}
        request={async (params) => {
          try {
            const page = await usersApi.listUsers({
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
            message.error(describeError(err, '加载用户列表失败'));
            return { data: [], total: 0, success: false };
          }
        }}
        locale={{
          emptyText: hasFilter ? (
            <EmptyStateWithAction variant="search-empty" onAction={clearSearch} />
          ) : (
            <EmptyStateWithAction
              variant="users-empty"
              onAction={() => setCreateOpen(true)}
            />
          ),
        }}
      />

      <CreateUserModal
        open={createOpen}
        onClose={() => setCreateOpen(false)}
        onCreated={reload}
      />

      <ResetPasswordModal
        open={resetPwd !== null}
        onClose={() => setResetPwd(null)}
        newPassword={resetPwd ?? ''}
      />
    </div>
  );
}
