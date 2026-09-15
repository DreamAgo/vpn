/**
 * 网段（CIDR）多选输入：tags 模式 Select + 网段目录下拉 + 即时校验。
 *
 * 组路由 / 服务端 LAN / 节点路由三处编辑弹窗共用，避免各自重复 Select 接线与 CIDR 校验。
 * 作为受控 Form.Item，由外层 Form 提供 `name` 对应字段值。
 */
import { Form, Select, Space } from 'antd';
import type { ReactNode } from 'react';

import { useSubnetOptions } from '@/hooks/useSubnetOptions';
import { isValidCidr } from '@/utils/cidr';

interface Props {
  /** 绑定的表单字段名（如 routes / routedSubnets）。 */
  name: string;
  /** 字段标签。 */
  label: string;
  /** 字段下方说明（示例/提示）。 */
  extra?: ReactNode;
}

export function CidrRoutesSelect({ name, label, extra }: Props) {
  return (
    <Form.Item
      name={name}
      label={label}
      extra={extra}
      rules={[
        {
          validator: (_rule, value: string[] | undefined) => {
            const invalid = (value ?? []).filter((s) => !isValidCidr(s));
            return invalid.length > 0
              ? Promise.reject(new Error(`存在非法 CIDR：${invalid.join('、')}`))
              : Promise.resolve();
          },
        },
      ]}
    >
      <RoutesInput />
    </Form.Item>
  );
}

function RoutesInput({ value = [], onChange, id }: {
  value?: string[];
  onChange?: (value: string[]) => void;
  id?: string;
}) {
  const subnetOptions = useSubnetOptions();
  return (
    <Space direction="vertical" style={{ width: '100%' }}>
      <Select
        aria-label="添加网段组"
        value={null}
        showSearch
        optionFilterProp="label"
        placeholder="选择网段组，加入组内全部 CIDR"
        options={subnetOptions}
        style={{ width: '100%' }}
        onChange={(groupId: string) => {
          const group = subnetOptions.find((option) => option.value === groupId);
          if (group) onChange?.([...new Set([...value, ...group.cidrs])]);
        }}
      />
      <Select
        id={id}
        value={value}
        onChange={onChange}
        mode="tags"
        allowClear
        placeholder="已选 CIDR，可删除或手动输入"
        tokenSeparators={[',', '，', ' ', '\n', '\r', '\t']}
        style={{ width: '100%' }}
      />
    </Space>
  );
}
