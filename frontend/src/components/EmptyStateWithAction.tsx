/**
 * Story 3.10：空状态组件（带主行动按钮）。
 *
 * 两种内置变体：
 * - users-empty：列表无任何用户时，引导创建第一个用户。
 * - search-empty：搜索/筛选无结果时，引导清除搜索。
 *
 * 所有文案与按钮均可通过 props 覆盖。居中布局。
 */
import type { ReactNode } from 'react';
import { Button, Empty, Typography } from 'antd';
import { UsergroupAddOutlined, SearchOutlined } from '@ant-design/icons';

const { Text } = Typography;

interface VariantConfig {
  icon: ReactNode;
  title: string;
  description?: string;
  actionLabel: string;
}

const VARIANTS: Record<'users-empty' | 'search-empty', VariantConfig> = {
  'users-empty': {
    icon: <UsergroupAddOutlined style={{ fontSize: 56, color: '#bfbfbf' }} />,
    title: '还没有用户',
    description: '创建第一个用户开始使用',
    actionLabel: '+ 创建第一个用户',
  },
  'search-empty': {
    icon: <SearchOutlined style={{ fontSize: 56, color: '#bfbfbf' }} />,
    title: '没找到匹配的内容，试试其他关键词',
    actionLabel: '清除搜索',
  },
};

interface Props {
  variant?: 'users-empty' | 'search-empty';
  title?: string;
  description?: string;
  /** 覆盖默认按钮（传入完整 ReactNode）；不传则渲染默认主按钮并在点击时调用 onAction。 */
  action?: ReactNode;
  /** 默认按钮的点击回调。 */
  onAction?: () => void;
}

export function EmptyStateWithAction({
  variant = 'users-empty',
  title,
  description,
  action,
  onAction,
}: Props) {
  const cfg = VARIANTS[variant];
  const resolvedTitle = title ?? cfg.title;
  const resolvedDesc = description ?? cfg.description;

  const defaultButton =
    variant === 'users-empty' ? (
      <Button type="primary" onClick={onAction}>
        {cfg.actionLabel}
      </Button>
    ) : (
      <Button onClick={onAction}>{cfg.actionLabel}</Button>
    );

  return (
    <div
      style={{
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        justifyContent: 'center',
        padding: '48px 16px',
        textAlign: 'center',
      }}
    >
      <Empty
        image={cfg.icon}
        imageStyle={{ height: 'auto', marginBottom: 16 }}
        description={
          <div>
            <div style={{ fontSize: 16, fontWeight: 500, marginBottom: resolvedDesc ? 4 : 0 }}>
              {resolvedTitle}
            </div>
            {resolvedDesc && <Text type="secondary">{resolvedDesc}</Text>}
          </div>
        }
      >
        <div style={{ marginTop: 16 }}>{action ?? defaultButton}</div>
      </Empty>
    </div>
  );
}
