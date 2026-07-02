/**
 * 网段目录 API 客户端。集中维护命名网段(名称+CIDR),供各处下拉选择。
 */
import { http } from './http';
import type {
  CreateSubnetRequest,
  SubnetDto,
  UpdateSubnetRequest,
} from '@/types/api';

export const subnetsApi = {
  async listSubnets(): Promise<SubnetDto[]> {
    const res = await http.get<SubnetDto[]>('/admin/subnets');
    return res.data;
  },

  async createSubnet(req: CreateSubnetRequest): Promise<SubnetDto> {
    const res = await http.post<SubnetDto>('/admin/subnets', req);
    return res.data;
  },

  async updateSubnet(id: string, req: UpdateSubnetRequest): Promise<SubnetDto> {
    const res = await http.patch<SubnetDto>(`/admin/subnets/${id}`, req);
    return res.data;
  },

  async deleteSubnet(id: string): Promise<void> {
    await http.delete(`/admin/subnets/${id}`);
  },
};
