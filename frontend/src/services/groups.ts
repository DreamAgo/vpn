/**
 * 用户组管理 API 客户端。
 *
 * 组持有一组"可路由网段"(CIDR);成员的 VPN allowed_routes 由组决定(访问控制)。
 * 所有方法返回业务 data(axios 拦截器已解包 ApiResponse 并 camelCase 化)。
 */
import { http } from './http';
import type {
  AssignGroupRequest,
  CreateUserGroupRequest,
  UpdateUserGroupRequest,
  UserGroupDto,
} from '@/types/api';

export const groupsApi = {
  async listGroups(): Promise<UserGroupDto[]> {
    const res = await http.get<UserGroupDto[]>('/admin/groups');
    return res.data;
  },

  async createGroup(req: CreateUserGroupRequest): Promise<UserGroupDto> {
    const res = await http.post<UserGroupDto>('/admin/groups', req);
    return res.data;
  },

  async updateGroup(id: string, req: UpdateUserGroupRequest): Promise<UserGroupDto> {
    const res = await http.patch<UserGroupDto>(`/admin/groups/${id}`, req);
    return res.data;
  },

  async deleteGroup(id: string): Promise<void> {
    await http.delete(`/admin/groups/${id}`);
  },

  /** 全量设置用户所属组(空数组取消所有分组)。 */
  async setUserGroups(userId: string, req: AssignGroupRequest): Promise<void> {
    await http.put(`/admin/users/${userId}/groups`, req);
  },
};
