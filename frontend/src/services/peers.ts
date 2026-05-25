/**
 * 节点（peer）管理 API 客户端（Epic 5）。
 *
 * 所有方法返回业务 data（axios 拦截器已解包 ApiResponse）。
 */
import { http } from './http';
import type { AdminPeerQuery, AdminPeerView, Page } from '@/types/api';

export const peersApi = {
  async listAdminPeers(query: AdminPeerQuery): Promise<Page<AdminPeerView>> {
    const res = await http.get<Page<AdminPeerView>>('/admin/peers', { params: query });
    return res.data;
  },

  async forceRemovePeer(id: string): Promise<void> {
    await http.delete(`/admin/peers/${id}`);
  },
};
