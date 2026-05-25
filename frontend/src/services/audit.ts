/**
 * 审计日志 API 客户端（Epic 5）。
 *
 * 所有方法返回业务 data（axios 拦截器已解包 ApiResponse）。
 */
import { http } from './http';
import type { AuditLogDto, AuditLogQuery, Page } from '@/types/api';

export const auditApi = {
  async listAuditLogs(query: AuditLogQuery): Promise<Page<AuditLogDto>> {
    const res = await http.get<Page<AuditLogDto>>('/admin/audit-logs', { params: query });
    return res.data;
  },
};
