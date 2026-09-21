/**
 * 审计日志 API 客户端（Epic 5）。
 *
 * 所有方法返回业务 data（axios 拦截器已解包 ApiResponse）。
 */
import { http } from './http';
import type { AuditLogDto, AuditLogQuery, Page } from '@/types/api';

export const auditApi = {
  async health(): Promise<{failedWrites:number;failedTransactionWrites:number;droppedEvents:number}> {
    const res=await http.get('/admin/audit-health'); return res.data;
  },
  async listAuditLogs(query: AuditLogQuery): Promise<Page<AuditLogDto>> {
    const res = await http.get<Page<AuditLogDto>>('/admin/audit-logs', { params: query });
    return res.data;
  },
};

export const auditActionLabels: Record<string, string> = {
  login_success: '登录成功', login_failed: '登录失败', external_login_success: '外部登录成功',
  external_login_failed: '外部登录失败', logout: '退出登录', first_time_setup: '初始化管理员',
  change_password: '修改密码', user_create: '创建用户', user_update: '更新用户', user_delete: '删除用户',
  user_reset_password: '重置密码', peer_register: '节点注册', peer_delete: '节点注销',
  peer_force_remove: '强制下线', peer_heartbeat: '心跳（历史）',
  'network.dns.update': '更新 DNS 配置', 'network.settings.update': '更新网络配置',
  'network.routes.update': '更新服务器路由', 'user.groups.update': '更新用户所属组',
  'grant.expiry.update': '更新授权期限', 'integration.settings.update': '更新集成配置',
  'notification.settings.update': '更新通知配置', 'notification.test': '测试通知',
  'system.restart.request': '请求重启', 'backup.download': '下载备份', 'backup.restore': '恢复备份',
  'client_update.configure':'配置客户端更新', 'client_update.sync':'同步客户端更新', 'integration.approval.subscribe':'订阅飞书审批', 'integration.user.lookup':'查询飞书用户', 'integration.user.sync':'同步飞书用户', 'user.feishu.bind':'绑定飞书身份', 'user.feishu.sync':'同步用户身份',
  'api_key.create': '创建 API 密钥', 'api_key.delete': '撤销 API 密钥',
  'group.create': '创建用户组', 'group.update': '更新用户组', 'group.delete': '删除用户组',
  'subnet.create': '创建子网', 'subnet.update': '更新子网', 'subnet.delete': '删除子网',
  'peer.routes.update': '更新节点路由', 'peer.purge': '清理节点', 'auth.rejected': '认证拒绝',
};
