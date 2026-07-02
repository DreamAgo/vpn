/**
 * 把网段目录转成 Select 选项(label="名称（CIDR）", value=CIDR),
 * 供组路由 / 服务端 LAN / 节点路由等处的网段选择器直接复用。
 */
import { useQuery } from '@tanstack/react-query';
import { subnetsApi } from '@/services/subnets';

export interface SubnetOption {
  label: string;
  value: string;
}

export function useSubnetOptions(): SubnetOption[] {
  const { data } = useQuery({ queryKey: ['subnets'], queryFn: subnetsApi.listSubnets });
  return (data ?? []).map((s) => ({ label: `${s.name}（${s.cidr}）`, value: s.cidr }));
}
