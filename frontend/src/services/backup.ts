import { getAccessToken } from './http';

interface ApiEnvelope<T> {
  code: number;
  message: string;
  data: T | null;
}

export interface RestoreResult {
  restored_at: number;
  users: number;
  peers: number;
  user_groups: number;
  subnets: number;
  audit_logs: number;
  requires_restart: boolean;
}

function authHeaders(): HeadersInit {
  const token = getAccessToken();
  return token ? { Authorization: `Bearer ${token}` } : {};
}

export const backupApi = {
  async downloadBackup(): Promise<void> {
    const response = await fetch('/api/v1/admin/backup', {
      method: 'GET',
      credentials: 'include',
      headers: authHeaders(),
    });
    if (!response.ok) throw new Error(`下载备份失败: HTTP ${response.status}`);

    const blob = await response.blob();
    const disposition = response.headers.get('content-disposition') ?? '';
    const matched = disposition.match(/filename="([^"]+)"/);
    const filename = matched?.[1] ?? `yilian-backup-${Date.now()}.json`;

    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = filename;
    document.body.appendChild(a);
    a.click();
    a.remove();
    URL.revokeObjectURL(url);
  },

  async restoreBackup(archive: unknown): Promise<RestoreResult> {
    const response = await fetch('/api/v1/admin/backup/restore', {
      method: 'POST',
      credentials: 'include',
      headers: {
        ...authHeaders(),
        'Content-Type': 'application/json',
      },
      body: JSON.stringify(archive),
    });
    const envelope = (await response.json()) as ApiEnvelope<RestoreResult>;
    if (!response.ok || envelope.code !== 0 || !envelope.data) {
      throw new Error(envelope.message || `恢复失败: HTTP ${response.status}`);
    }
    return envelope.data;
  },
};
