/**
 * 把网段组转成 Select 选项，保留组 ID 和组内全部 CIDR，
 * 供组路由 / 服务端 LAN / 节点路由等处的网段选择器直接复用。
 */
import { useQuery } from '@tanstack/react-query';
import { parseCidrText } from '@/utils/cidr';
import { subnetsApi } from '@/services/subnets';

export interface SubnetOption {
  label: string;
  value: string;
  cidrs: string[];
}

export function useSubnetOptions(): SubnetOption[] {
  const { data } = useQuery({ queryKey: ['subnets'], queryFn: subnetsApi.listSubnets });
  return (data ?? []).map((s) => ({
    label: `${s.name}（${(s.cidrs ?? parseCidrText(s.cidr)).length} 个网段）`,
    value: s.id,
    cidrs: s.cidrs ?? parseCidrText(s.cidr),
  }));
}
