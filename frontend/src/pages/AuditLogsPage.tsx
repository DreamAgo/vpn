/**
 * Story 5.8：审计日志页（ProTable）。
 *
 * - ProTable request 模式调 GET /admin/audit-logs（分页）。
 * - 筛选：时间范围 RangePicker（默认最近 7 天）+ 类型下拉 + 用户名搜索。
 * - 筛选条件同步到 URL query（useSearchParams），便于分享。
 * - 列：时间（绝对 + 相对悬停）/ 用户 / 类型（Tag 着色）/ 资源 / 状态码。
 * - 展开行显示完整 metadata（JSON.parse 后美化，失败则原样显示）。
 */
import { useCallback, useMemo, useRef, useState } from 'react';
import { Input, Select, DatePicker, Space, Tag, Typography, App } from 'antd';
import { ProTable, type ActionType, type ProColumns } from '@ant-design/pro-components';
import { useSearchParams } from 'react-router-dom';
import dayjs from 'dayjs';
import relativeTime from 'dayjs/plugin/relativeTime';
import 'dayjs/locale/zh-cn';

import { auditApi } from '@/services/audit';
import { ApiError } from '@/services/http';
import type { AuditLogDto } from '@/types/api';
import { codeFontFamily } from '@/theme';

dayjs.extend(relativeTime);
dayjs.locale('zh-cn');

const { Title, Text } = Typography;
const { RangePicker } = DatePicker;

/** action → Tag 颜色。未知类型用默认色。 */
function actionColor(action: string): string {
  const a = action.toLowerCase();
  if (a.includes('login') || a.includes('logout') || a.includes('auth')) return 'blue';
  if (a.includes('create') || a.includes('register') || a.includes('add')) return 'green';
  if (a.includes('delete') || a.includes('remove') || a.includes('force')) return 'red';
  if (a.includes('update') || a.includes('change') || a.includes('reset') || a.includes('patch'))
    return 'orange';
  return 'default';
}

function describeError(err: unknown, fallback: string): string {
  if (err instanceof ApiError) return err.message || fallback;
  return fallback;
}

/** 美化 metadata：JSON 则缩进展示，否则原样。 */
function MetadataView({ metadata }: { metadata: string | null }) {
  if (!metadata) return <Text type="secondary">无附加信息</Text>;
  let display: string;
  try {
    display = JSON.stringify(JSON.parse(metadata), null, 2);
  } catch {
    display = metadata;
  }
  return (
    <pre
      style={{
        fontFamily: codeFontFamily,
        fontSize: 12,
        margin: 0,
        whiteSpace: 'pre-wrap',
        wordBreak: 'break-all',
        background: '#FAFAFA',
        padding: 12,
        borderRadius: 4,
      }}
    >
      {display}
    </pre>
  );
}

const DEFAULT_DAYS = 7;

export function AuditLogsPage() {
  const { message } = App.useApp();
  const actionRef = useRef<ActionType>(null);
  const [searchParams, setSearchParams] = useSearchParams();

  // 从 URL 初始化筛选状态（默认最近 7 天）。
  const initFrom = searchParams.get('from');
  const initTo = searchParams.get('to');
  const [range, setRange] = useState<[dayjs.Dayjs, dayjs.Dayjs] | null>(() => {
    if (initFrom && initTo) {
      return [dayjs(Number(initFrom)), dayjs(Number(initTo))];
    }
    return [dayjs().subtract(DEFAULT_DAYS, 'day').startOf('day'), dayjs().endOf('day')];
  });
  const [action, setAction] = useState<string | undefined>(
    searchParams.get('action') ?? undefined
  );
  const [usernameInput, setUsernameInput] = useState(searchParams.get('username') ?? '');
  const [username, setUsername] = useState<string>(searchParams.get('username') ?? '');
  const debounceTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const reload = useCallback(() => actionRef.current?.reload(), []);

  // 将当前筛选写回 URL。
  const syncUrl = useCallback(
    (next: {
      range?: [dayjs.Dayjs, dayjs.Dayjs] | null;
      action?: string;
      username?: string;
    }) => {
      const r = next.range !== undefined ? next.range : range;
      const a = next.action !== undefined ? next.action : action;
      const u = next.username !== undefined ? next.username : username;
      const params: Record<string, string> = {};
      if (r) {
        params.from = String(r[0].valueOf());
        params.to = String(r[1].valueOf());
      }
      if (a) params.action = a;
      if (u) params.username = u;
      setSearchParams(params, { replace: true });
    },
    [range, action, username, setSearchParams]
  );

  const handleUsernameChange = (value: string) => {
    setUsernameInput(value);
    if (debounceTimer.current) clearTimeout(debounceTimer.current);
    debounceTimer.current = setTimeout(() => {
      const v = value.trim();
      setUsername(v);
      syncUrl({ username: v });
      reload();
    }, 300);
  };

  const columns = useMemo<ProColumns<AuditLogDto>[]>(
    () => [
      {
        title: '时间',
        dataIndex: 'createdAt',
        width: 180,
        render: (_, record) => (
          <span title={dayjs(record.createdAt).fromNow()}>
            {dayjs(record.createdAt).format('YYYY-MM-DD HH:mm:ss')}
          </span>
        ),
      },
      {
        title: '用户',
        dataIndex: 'username',
        width: 140,
        ellipsis: true,
        render: (_, record) => record.username ?? <Text type="secondary">—</Text>,
      },
      {
        title: '类型',
        dataIndex: 'action',
        width: 160,
        render: (_, record) => <Tag color={actionColor(record.action)}>{record.action}</Tag>,
      },
      {
        title: '资源',
        dataIndex: 'resource',
        ellipsis: true,
      },
      {
        title: '状态码',
        dataIndex: 'statusCode',
        width: 90,
        render: (_, record) =>
          record.statusCode != null ? (
            <Tag color={record.statusCode < 400 ? 'success' : 'error'}>{record.statusCode}</Tag>
          ) : (
            <Text type="secondary">—</Text>
          ),
      },
    ],
    []
  );

  return (
    <div style={{ padding: 24 }}>
      <Title level={4} style={{ marginBottom: 16 }}>
        审计日志
      </Title>

      <ProTable<AuditLogDto>
        actionRef={actionRef}
        rowKey="id"
        columns={columns}
        search={false}
        options={{ reload: true, density: false, setting: false }}
        pagination={{ defaultPageSize: 20, showSizeChanger: true }}
        expandable={{
          expandedRowRender: (record) => <MetadataView metadata={record.metadata} />,
        }}
        toolbar={{
          search: (
            <Space wrap>
              <RangePicker
                showTime
                allowClear={false}
                value={range}
                style={{ width: 360 }}
                onChange={(v) => {
                  const next = v && v[0] && v[1] ? ([v[0], v[1]] as [dayjs.Dayjs, dayjs.Dayjs]) : null;
                  setRange(next);
                  syncUrl({ range: next });
                  reload();
                }}
              />
              <Select
                allowClear
                placeholder="类型"
                value={action}
                style={{ width: 160 }}
                onChange={(v) => {
                  setAction(v);
                  syncUrl({ action: v });
                  reload();
                }}
                options={[
                  { value: 'login', label: '登录' },
                  { value: 'logout', label: '登出' },
                  { value: 'user.create', label: '创建用户' },
                  { value: 'user.update', label: '更新用户' },
                  { value: 'user.delete', label: '删除用户' },
                  { value: 'peer.register', label: '节点注册' },
                  { value: 'peer.force_remove', label: '强制下线' },
                  { value: 'password.change', label: '修改密码' },
                  { value: 'password.reset', label: '重置密码' },
                ]}
              />
              <Input.Search
                allowClear
                placeholder="搜索用户名"
                value={usernameInput}
                onChange={(e) => handleUsernameChange(e.target.value)}
                style={{ width: 200 }}
              />
            </Space>
          ),
        }}
        request={async (params) => {
          try {
            const page = await auditApi.listAuditLogs({
              page: params.current,
              pageSize: params.pageSize,
              from: range ? range[0].valueOf() : undefined,
              to: range ? range[1].valueOf() : undefined,
              action: action || undefined,
              username: username || undefined,
            });
            return {
              data: page.items,
              total: page.total,
              success: true,
            };
          } catch (err) {
            message.error(describeError(err, '加载审计日志失败'));
            return { data: [], total: 0, success: false };
          }
        }}
      />
    </div>
  );
}
