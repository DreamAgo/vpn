import { http } from './http';
import type { ApiKeyDto, CreateApiKeyRequest, CreateApiKeyResponse } from '@/types/api';

export const apiKeysApi = {
  async listApiKeys(): Promise<ApiKeyDto[]> {
    const res = await http.get<ApiKeyDto[]>('/admin/api-keys');
    return res.data;
  },

  async createApiKey(req: CreateApiKeyRequest): Promise<CreateApiKeyResponse> {
    const res = await http.post<CreateApiKeyResponse>('/admin/api-keys', req);
    return res.data;
  },

  async revokeApiKey(id: string): Promise<void> {
    await http.delete(`/admin/api-keys/${id}`);
  },
};
