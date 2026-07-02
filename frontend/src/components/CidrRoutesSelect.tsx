/**
 * 网段（CIDR）多选输入：tags 模式 Select + 网段目录下拉 + 即时校验。
 *
 * 组路由 / 服务端 LAN / 节点路由三处编辑弹窗共用，避免各自重复 Select 接线与 CIDR 校验。
 * 作为受控 Form.Item，由外层 Form 提供 `name` 对应字段值。
 */
import { Form, Select } from 'antd';
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
  const subnetOptions = useSubnetOptions();
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
      <Select
        mode="tags"
        allowClear
        showSearch
        optionFilterProp="label"
        placeholder="从网段目录选择，或手动输入 CIDR 后回车"
        tokenSeparators={[',', ' ', '\n']}
        options={subnetOptions}
        style={{ width: '100%' }}
      />
    </Form.Item>
  );
}
