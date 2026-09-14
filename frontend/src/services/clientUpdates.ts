import { http } from './http';
export interface ClientUpdateStatus {
  autoSync: boolean;
  publicBaseUrl: string;
  lastCheckedAt: number | null;
  lastSyncedAt: number | null;
  lastError: string | null;
  repository: string;
  syncing: boolean;
  nextCheckAt: number | null;
  manifest: {
    version: string;
    notes: string;
    pubDate?: string;
    downloads: { name: string; url: string; size: number; digest: string }[];
  } | null;
}
export const clientUpdatesApi = {
  async status() { return (await http.get<ClientUpdateStatus>('/admin/client-updates')).data; },
  async save(autoSync: boolean, publicBaseUrl: string) {
    return (await http.put<ClientUpdateStatus>('/admin/client-updates', { autoSync, publicBaseUrl })).data;
  },
  async sync() { return (await http.post<ClientUpdateStatus>('/admin/client-updates/sync')).data; },
};
