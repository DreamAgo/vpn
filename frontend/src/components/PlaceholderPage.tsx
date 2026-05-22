/**
 * 占位页（Story 1.7）：4 个主导航页面在 Epic 2-5 真实实现前的占位。
 */
import { Result } from 'antd';

interface Props {
  name: string;
}

export function PlaceholderPage({ name }: Props) {
  return (
    <Result
      status="info"
      title={`${name} - 即将到来`}
      subTitle={`此页面将在后续 Story 中实现。当前 Story 1.7 仅完成主框架与设计系统配置。`}
    />
  );
}
