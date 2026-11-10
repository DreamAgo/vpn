import { ApiError, http } from './http';

export interface ClusterGateway {
  id: string;
  name: string;
  probeEndpoint: string;
  ready: boolean;
  isDefault: boolean;
}

export interface DeviceGatewayPolicy {
  grantedGatewayIds: string[];
  targetGatewayId: string | null;
  revision: number;
}

export interface DeviceGatewayState extends DeviceGatewayPolicy {
  supported: boolean;
  activeGatewayId: string | null;
  confirmedGatewayId: string | null;
  confirmedRevision: number | null;
}

export const clusterGatewaysQueryKey = ['cluster-gateways'] as const;

export const clusterApi = {
  async listGateways(): Promise<ClusterGateway[] | null> {
    try {
      const res = await http.get<ClusterGateway[]>('/admin/cluster/gateways');
      return res.data;
    } catch (err) {
      if (err instanceof ApiError && err.status === 404) return null;
      throw err;
    }
  },

  async setDefaultGateway(gatewayId: string): Promise<void> {
    await http.put('/admin/cluster/default', { gatewayId });
  },

  async getDeviceGateway(peerId: string): Promise<DeviceGatewayState> {
    const res = await http.get<DeviceGatewayState>(`/admin/cluster/devices/${encodeURIComponent(peerId)}`);
    return res.data;
  },

  async updateDeviceGateway(peerId: string, policy: DeviceGatewayPolicy): Promise<DeviceGatewayState> {
    const res = await http.put<DeviceGatewayState>(`/admin/cluster/devices/${encodeURIComponent(peerId)}`, policy);
    return res.data;
  },
};
