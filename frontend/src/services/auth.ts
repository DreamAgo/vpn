/**
 * 认证相关 API 客户端。
 *
 * 所有方法返回业务 data（axios 拦截器已解包 ApiResponse）。
 */
import { http } from './http';
import type {
  ChangePasswordRequest,
  EmailNotificationSettings,
  FirstTimeSetupRequest,
  FirstTimeSetupResponse,
  LoginRequest,
  LoginResponse,
  LogoutRequest,
  NotificationEventQuery,
  NotificationEventView,
  RefreshResponse,
  SetupStatusResponse,
  SystemInfo,
  TestEmailNotificationRequest,
  UpdateEmailNotificationSettingsRequest,
} from '@/types/api';

export const authApi = {
  async getSetupStatus(): Promise<SetupStatusResponse> {
    const res = await http.get<SetupStatusResponse>('/auth/setup-status');
    return res.data;
  },

  async firstTimeSetup(req: FirstTimeSetupRequest): Promise<FirstTimeSetupResponse> {
    const res = await http.post<FirstTimeSetupResponse>('/auth/first-time-setup', req);
    return res.data;
  },

  async login(req: LoginRequest): Promise<LoginResponse> {
    const res = await http.post<LoginResponse>('/auth/login', req);
    return res.data;
  },

  async refresh(refreshToken: string): Promise<RefreshResponse> {
    const res = await http.post<RefreshResponse>('/auth/refresh', { refreshToken });
    return res.data;
  },

  async logout(refreshToken: string): Promise<void> {
    const body: LogoutRequest = { refreshToken };
    await http.post('/auth/logout', body);
  },

  async changePassword(req: ChangePasswordRequest): Promise<void> {
    await http.post('/auth/change-password', req);
  },
};

export const systemApi = {
  async getSystemInfo(): Promise<SystemInfo> {
    const res = await http.get<SystemInfo>('/admin/system/info');
    return res.data;
  },

  /** 更新服务端 LAN 网段（PUT /admin/system/routes），返回规整后的网段。 */
  async updateServerRoutes(routes: string[]): Promise<string[]> {
    const res = await http.put<string[]>('/admin/system/routes', { routes });
    return res.data;
  },

  async getEmailNotificationSettings(): Promise<EmailNotificationSettings> {
    const res = await http.get<EmailNotificationSettings>('/admin/notifications/email');
    return res.data;
  },

  async updateEmailNotificationSettings(
    req: UpdateEmailNotificationSettingsRequest
  ): Promise<EmailNotificationSettings> {
    const res = await http.put<EmailNotificationSettings>('/admin/notifications/email', req);
    return res.data;
  },

  async sendTestEmail(req: TestEmailNotificationRequest): Promise<void> {
    await http.post('/admin/notifications/email/test', req);
  },

  async listNotificationEvents(
    query: NotificationEventQuery = {}
  ): Promise<NotificationEventView[]> {
    const res = await http.get<NotificationEventView[]>('/admin/notifications/events', {
      params: query,
    });
    return res.data;
  },
};
