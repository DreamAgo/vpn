/**
 * 用户管理 API 客户端（Epic 3）。
 *
 * 所有方法返回业务 data（axios 拦截器已解包 ApiResponse）。
 */
import { http } from './http';
import type {
  CreateUserRequest,
  CreateUserResponse,
  ListUsersQuery,
  Page,
  ResetPasswordResponse,
  UpdateUserRequest,
  UserDto,
} from '@/types/api';

export const usersApi = {
  async createUser(req: CreateUserRequest): Promise<CreateUserResponse> {
    const res = await http.post<CreateUserResponse>('/admin/users', req);
    return res.data;
  },

  async listUsers(query: ListUsersQuery): Promise<Page<UserDto>> {
    const res = await http.get<Page<UserDto>>('/admin/users', { params: query });
    return res.data;
  },

  async updateUser(id: string, req: UpdateUserRequest): Promise<UserDto> {
    const res = await http.patch<UserDto>(`/admin/users/${id}`, req);
    return res.data;
  },

  async resetPassword(id: string): Promise<ResetPasswordResponse> {
    const res = await http.post<ResetPasswordResponse>(`/admin/users/${id}/reset-password`);
    return res.data;
  },

  async deleteUser(id: string): Promise<void> {
    await http.delete(`/admin/users/${id}`);
  },
};
