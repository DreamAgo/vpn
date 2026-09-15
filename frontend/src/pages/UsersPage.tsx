/**
 * Story 3.6：用户管理页（ProTable）。
 *
 * - ProTable request 模式调 GET /admin/users（分页 + 搜索 + 状态筛选）。
 * - 搜索框防抖 300ms；状态下拉（全部/正常/已禁用）。
 * - 操作列：重置密码 / 启用·禁用 / 删除（均带确认，成功后刷新 + message）。
 * - 空态：无任何用户 → users-empty；搜索无结果 → search-empty。
 */
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
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
import { useQuery } from '@tanstack/react-query';
import dayjs from 'dayjs';
import relativeTime from 'dayjs/plugin/relativeTime';
import 'dayjs/locale/zh-cn';

import { GrantExpiryButton } from '@/components/GrantExpiryButton';
import { FeishuBindingModal, feishuStatusLabels } from '@/components/FeishuBindingModal';
import { usersApi } from '@/services/users';
import { groupsApi } from '@/services/groups';
import { ApiError } from '@/services/http';
import { ErrorCodes } from '@/types/api';
import type { UserDto } from '@/types/api';
import { CreateUserModal } from '@/components/CreateUserModal';
import { ResetPasswordModal } from '@/components/ResetPasswordModal';
import { AssignGroupModal } from '@/components/AssignGroupModal';
import { SetMaxDevicesModal } from '@/components/SetMaxDevicesModal';
import { EmptyStateWithAction } from '@/components/EmptyStateWithAction';

/** 渲染某用户所属的多个组名（从 groups 查询缓存解析）。 */
function GroupTags({ groupIds }: { groupIds: string[] }) {
  const { data: groups } = useQuery({ queryKey: ['groups'], queryFn: groupsApi.listGroups });
  if (!groupIds || groupIds.length === 0)
    return <Typography.Text type="secondary">未分组</Typography.Text>;
  return (
    <Space size={[0, 4]} wrap>
      {groupIds.map((id) => {
        const g = groups?.find((x) => x.id === id);
        return (
          <Tag key={id} color="blue">
            {g ? g.name : '未知组'}
          </Tag>
        );
      })}
    </Space>
  );
}

/** 独占截止时刻按上海时区展示，避免浏览器所在时区改变审批日期。 */
const expiryFormatter = new Intl.DateTimeFormat('zh-CN', {
  timeZone: 'Asia/Shanghai', year: 'numeric', month: '2-digit', day: '2-digit',
  hour: '2-digit', minute: '2-digit', second: '2-digit', hourCycle: 'h23',
});

function ApprovalAccess({ user, onSaved }: { user: UserDto; onSaved: () => void }) {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, []);
  if (!user.accessMode || !user.approvalGrants) {
    return <Typography.Text type="secondary">服务端未提供授权信息</Typography.Text>;
  }
  return (
    <Space direction="vertical" size={8} className="approval-access-cell">
      <Tag>{user.accessMode === 'approval_required' ? '审批管控' : '历史权限规则'}</Tag>
      {user.approvalGrants.length === 0 && (
        <Typography.Text type="secondary">暂无审批授权</Typography.Text>
      )}
      {user.approvalGrants.map((grant) => (
        <div key={grant.groupId} className="approval-grant">
          <div>
            <Tag color={grant.expiresAt > now ? 'success' : 'error'}>
              {grant.expiresAt > now ? '有效' : '已到期'}
            </Tag>
            {grant.groupName}
          </div>
          <Typography.Text type="secondary">
            {expiryFormatter.format(grant.expiresAt)} 到期
            <GrantExpiryButton userId={user.id} groupId={grant.groupId} groupName={grant.groupName}
              expiresAt={grant.expiresAt} onSaved={onSaved} />
          </Typography.Text>
        </div>
      ))}
      {user.groupIds.length > 0 && (
        <Typography.Text type="secondary">人工分组不设到期，仍按账号状态生效</Typography.Text>
      )}
    </Space>
  );
}

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

  const [feishuUser, setFeishuUser] = useState<UserDto | null>(null);
  const [feishuSyncing, setFeishuSyncing] = useState(false);
  const [createOpen, setCreateOpen] = useState(false);
  const [resetPwd, setResetPwd] = useState<string | null>(null);
  const [assignUser, setAssignUser] = useState<UserDto | null>(null);
  const [maxDevUser, setMaxDevUser] = useState<UserDto | null>(null);

  const reload = useCallback(() => actionRef.current?.reload(), []);

  const syncFeishu = async (user: UserDto) => {
    setFeishuSyncing(true);
    try { await usersApi.syncFeishu(user.id); message.success('飞书状态已同步'); }
    catch (error) { message.error(describeError(error, '同步失败')); }
    finally { setFeishuSyncing(false); reload(); }
  };
  const syncAllFeishu = async () => {
    setFeishuSyncing(true);
    try { const count = await usersApi.syncAllFeishu(); message.success(`已安排 ${count} 个飞书身份后台同步，请稍后刷新`); }
    catch (error) { message.error(describeError(error, '同步失败')); }
    finally { setFeishuSyncing(false); reload(); }
  };

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

  const userFeishuAction = (user: UserDto) => user.feishuBindings?.length
    ? <span onClick={() => void syncFeishu(user)}>同步飞书状态</span>
    : <span onClick={() => setFeishuUser(user)}>绑定飞书用户</span>;

  // 是否处于"有筛选条件"状态（决定空态变体）。
  const hasFilter = search.length > 0 || statusFilter !== undefined;

  const columns = useMemo<ProColumns<UserDto>[]>(
    () => [
      {
        title: '用户 / 邮箱',
        dataIndex: 'username',
        width: 200,
        render: (_, user) => <div className="user-identity-cell">
          <Typography.Text strong>{user.username}</Typography.Text>
          <Typography.Text type="secondary">{user.email}</Typography.Text>
        </div>,
      },
      {
        title: '状态',
        dataIndex: 'status',
        width: 80,
        render: (_, record) =>
          record.status === 'active' ? (
            <Tag color="success">正常</Tag>
          ) : (
            <Tag color="default">已禁用</Tag>
          ),
      },
      {
        title: '飞书账号 / 状态', key: 'feishu', width: 210,
        render: (_, user) => user.feishuBindings?.length ? <Space direction="vertical" size={4}>
          {user.feishuBindings.map(binding => <div key={binding.unionId}>
            <div>{binding.name || '已绑定'} <Tag color={binding.blocked ? 'error' : binding.status === 'active' ? 'success' : 'default'}>{feishuStatusLabels[binding.status] || binding.status}</Tag></div>
            <Typography.Text type="secondary">{binding.email}</Typography.Text>
            <div><Typography.Text type="secondary">{binding.syncedAt ? `同步于 ${dayjs(binding.syncedAt).format('MM-DD HH:mm:ss')}` : '尚未同步'}</Typography.Text></div>
            {binding.lastError && <Typography.Paragraph type="danger" style={{ marginBottom: 0, fontSize: 12 }} ellipsis={{ rows: 2, tooltip: binding.lastError }}>{binding.lastError}</Typography.Paragraph>}
          </div>)}
        </Space> : <Typography.Text type="secondary">未绑定</Typography.Text>,
      },
      {
        title: '人工分组',
        dataIndex: 'groupIds',
        width: 130,
        render: (_, record) => <GroupTags groupIds={record.groupIds} />,
      },
      {
        title: '审批授权 · 北京时间',
        key: 'approvalAccess',
        width: 310,
        render: (_, record) => <ApprovalAccess user={record} onSaved={reload} />,
      },
      {
        title: '终端上限',
        dataIndex: 'maxDevices',
        width: 90,
        render: (_, record) => (
          <Tag color={record.maxDevices > 1 ? 'blue' : 'default'}>{record.maxDevices}</Tag>
        ),
      },
      {
        title: '最后登录',
        dataIndex: 'lastLoginAt',
        width: 110,
        responsive: ['xl'],
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
        width: 104,
        fixed: 'right',
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
                    key: 'feishu',
                    label: userFeishuAction(record),
                  },
                  {
                    key: 'assign-group',
                    label: <span onClick={() => setAssignUser(record)}>分配用户组</span>,
                  },
                  {
                    key: 'max-devices',
                    label: <span onClick={() => setMaxDevUser(record)}>设置终端上限</span>,
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
              <Button type="text" size="small" aria-label={`更多操作：${record.username}`} icon={<MoreOutlined />} />
            </Dropdown>
          </Space>
        ),
      },
    ],
    // eslint-disable-next-line react-hooks/exhaustive-deps
    []
  );

  return (
    <div>
      <div className="page-heading">
        <div><Title level={4} style={{ margin: 0 }}>用户管理</Title>
          <Typography.Text type="secondary">管理账号、飞书身份和网络访问授权</Typography.Text></div>
        <Button onClick={syncAllFeishu} loading={feishuSyncing}>同步飞书用户</Button>
      </div>
      {feishuUser && <FeishuBindingModal key={feishuUser.id} user={feishuUser} onClose={() => setFeishuUser(null)} onSuccess={reload} />}
      <ProTable<UserDto>
        actionRef={actionRef}
        rowKey="id"
        columns={columns}
        scroll={{ x: 1234 }}
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

      <AssignGroupModal
        open={assignUser !== null}
        user={assignUser}
        onClose={() => setAssignUser(null)}
        onSaved={reload}
      />

      <SetMaxDevicesModal
        open={maxDevUser !== null}
        user={maxDevUser}
        onClose={() => setMaxDevUser(null)}
        onSaved={reload}
      />
    </div>
  );
}
