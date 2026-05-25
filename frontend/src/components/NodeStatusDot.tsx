/**
 * Story 5.6：节点状态指示点（纯展示组件，无网络）。
 *
 * - 色点 + 中文文字，颜色按状态语义着色。
 * - 传入 lastSeen 时悬停显示"最后心跳：X 分钟前"（dayjs fromNow）。
 * - 提供 aria-label（如"节点状态：在线"）便于无障碍访问。
 */
import { Tooltip } from 'antd';
import dayjs from 'dayjs';
import relativeTime from 'dayjs/plugin/relativeTime';
import 'dayjs/locale/zh-cn';

dayjs.extend(relativeTime);
dayjs.locale('zh-cn');

export type NodeStatus =
  | 'online'
  | 'offline'
  | 'connecting'
  | 'error'
  | 'deleted'
  | 'force_removed';

interface StatusConfig {
  color: string;
  label: string;
}

const STATUS_CONFIG: Record<NodeStatus, StatusConfig> = {
  online: { color: '#52C41A', label: '在线' },
  connecting: { color: '#FAAD14', label: '连接中' },
  offline: { color: '#BFBFBF', label: '离线' },
  error: { color: '#FF4D4F', label: '异常' },
  force_removed: { color: '#FF4D4F', label: '已下线' },
  deleted: { color: '#BFBFBF', label: '已删除' },
};

const DOT_SIZE: Record<'sm' | 'md' | 'lg', number> = { sm: 6, md: 8, lg: 12 };
const FONT_SIZE: Record<'sm' | 'md' | 'lg', number> = { sm: 12, md: 14, lg: 16 };

interface Props {
  status: NodeStatus | string;
  lastSeen?: number | null;
  size?: 'sm' | 'md' | 'lg';
}

export function NodeStatusDot({ status, lastSeen, size = 'md' }: Props) {
  const cfg = STATUS_CONFIG[status as NodeStatus] ?? { color: '#BFBFBF', label: status };
  const dot = DOT_SIZE[size];
  const font = FONT_SIZE[size];

  const content = (
    <span
      aria-label={`节点状态：${cfg.label}`}
      style={{ display: 'inline-flex', alignItems: 'center', gap: 6 }}
    >
      <span
        style={{
          display: 'inline-block',
          width: dot,
          height: dot,
          borderRadius: '50%',
          backgroundColor: cfg.color,
          flexShrink: 0,
        }}
      />
      <span style={{ fontSize: font }}>{cfg.label}</span>
    </span>
  );

  if (lastSeen != null) {
    return <Tooltip title={`最后心跳：${dayjs(lastSeen).fromNow()}`}>{content}</Tooltip>;
  }

  return content;
}
