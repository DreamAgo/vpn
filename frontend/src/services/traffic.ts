import { http } from './http';

export interface TrafficBytes {
  uploadBytes: number;
  downloadBytes: number;
}
export interface TrafficPoint extends TrafficBytes {
  timestamp: number;
}
export interface PeerTraffic extends TrafficBytes {
  peerId: string;
  deviceName: string;
  username: string;
  vpnIp: string;
  uploadBps: number | null;
  downloadBps: number | null;
  sampledAt: number;
}
export interface TrafficDashboard {
  from: number;
  to: number;
  firstSampleAt: number | null;
  sources: { id: string; name: string; sampledAt: number | null }[];
  period: TrafficBytes;
  today: TrafficBytes;
  total: TrafficBytes;
  uploadBps: number | null;
  downloadBps: number | null;
  series: TrafficPoint[];
  peers: PeerTraffic[];
}
export const trafficApi = {
  async dashboard(days: number): Promise<TrafficDashboard> {
    const response = await http.get<TrafficDashboard>('/admin/traffic', { params: { days } });
    return response.data;
  },
};

export function formatTraffic(bytes: number | null | undefined, rate = false): string {
  if (bytes == null) return '—';
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB', 'PiB'];
  const index = Math.min(Math.floor(Math.log2(Math.max(1, bytes)) / 10), units.length - 1);
  return `${(bytes / 1024 ** index).toLocaleString('zh-CN', { maximumFractionDigits: index ? 2 : 0 })} ${units[index]}${rate ? '/s' : ''}`;
}
