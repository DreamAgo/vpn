import { Alert, App, Button, Descriptions, Form, Modal, Select, Spin, Tag } from 'antd';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';

import {
  clusterApi,
  clusterGatewaysQueryKey,
  type ClusterGateway,
  type DeviceGatewayPolicy,
  type DeviceGatewayState,
} from '@/services/cluster';
import { ApiError } from '@/services/http';
import type { AdminPeerView } from '@/types/api';

interface Props {
  peer: AdminPeerView;
  gateways: ClusterGateway[];
  onClose: () => void;
}

function switchStatus(state: DeviceGatewayState, target: ClusterGateway | undefined) {
  if (!state.supported) return <Tag>客户端版本暂不支持远程切换</Tag>;

  const confirmed = state.activeGatewayId !== null
    && state.activeGatewayId === state.confirmedGatewayId
    && state.confirmedRevision === state.revision;
  if (state.targetGatewayId === null) {
    return confirmed ? <Tag color="green">自动选路已连接</Tag> : <Tag>等待客户端连接或确认</Tag>;
  }
  if (confirmed && state.activeGatewayId === state.targetGatewayId) {
    return <Tag color="green">已连接指定网关</Tag>;
  }
  if (!target?.ready) return <Tag color="orange">目标未就绪，等待切换</Tag>;
  if (state.activeGatewayId === state.targetGatewayId) {
    return <Tag color="processing">等待客户端确认握手</Tag>;
  }
  return <Tag color="processing">等待客户端切换</Tag>;
}

export function EditPeerGatewayModal({ peer, gateways, onClose }: Props) {
  const { message } = App.useApp();
  const queryClient = useQueryClient();
  const [draft, setDraft] = useState<DeviceGatewayPolicy | null>(null);
  const queryKey = ['cluster-device', peer.id];
  const device = useQuery({
    queryKey,
    queryFn: () => clusterApi.getDeviceGateway(peer.id),
    refetchInterval: 5_000,
  });
  const state = device.data;
  const form = draft ?? state;
  const revisionChanged = draft !== null && state?.revision !== draft.revision;

  const save = useMutation({
    mutationFn: (policy: DeviceGatewayPolicy) => clusterApi.updateDeviceGateway(peer.id, policy),
    onSuccess: async (updated) => {
      await queryClient.cancelQueries({ queryKey });
      queryClient.setQueryData(queryKey, updated);
      setDraft(null);
      message.success('网关设置已保存');
    },
    onError: async (err) => {
      message.error(err instanceof ApiError ? err.message : '保存网关设置失败');
      if (err instanceof ApiError && err.status === 409) {
        await queryClient.invalidateQueries({ queryKey });
        await queryClient.invalidateQueries({ queryKey: clusterGatewaysQueryKey });
      }
    },
  });

  const edit = (updates: Partial<Omit<DeviceGatewayPolicy, 'revision'>>) => {
    if (!form) return;
    setDraft({
      grantedGatewayIds: form.grantedGatewayIds,
      targetGatewayId: form.targetGatewayId,
      revision: form.revision,
      ...updates,
    });
  };

  const defaultGateway = gateways.find((gateway) => gateway.isDefault);
  const target = gateways.find((gateway) => gateway.id === state?.targetGatewayId);
  const gatewayName = (id: string | null) => id === null
    ? '—'
    : (gateways.find((gateway) => gateway.id === id)?.name ?? id);
  const option = (gateway: ClusterGateway) => ({
    value: gateway.id,
    label: `${gateway.name}${gateway.isDefault ? '（默认）' : ''}${gateway.ready ? '' : ' · 未就绪'}`,
  });
  const close = () => {
    if (!save.isPending) onClose();
  };
  const changed = draft !== null && state !== undefined && (
    draft.targetGatewayId !== state.targetGatewayId
    || draft.grantedGatewayIds.length !== state.grantedGatewayIds.length
    || draft.grantedGatewayIds.some((id) => !state.grantedGatewayIds.includes(id))
  );

  return (
    <Modal
      open
      title={`连接网关 · ${peer.deviceName}`}
      onCancel={close}
      maskClosable={false}
      confirmLoading={save.isPending}
      okText="保存"
      cancelText="关闭"
      okButtonProps={{ disabled: !changed || revisionChanged || !state?.supported || device.isError }}
      onOk={() => {
        if (draft && !revisionChanged) save.mutate(draft);
      }}
    >
      {!state && device.isPending && <Spin />}
      {device.isError && (
        <Alert
          type="error"
          showIcon
          message="读取设备网关状态失败"
          description={device.error instanceof ApiError ? device.error.message : undefined}
          action={<Button size="small" loading={device.isFetching} onClick={() => void device.refetch()}>重试</Button>}
          style={{ marginBottom: 16 }}
        />
      )}
      {state && form && (
        <>
          <Descriptions column={1} bordered size="small" style={{ marginBottom: 16 }}>
            <Descriptions.Item label="指定网关">
              {state.targetGatewayId === null ? '自动选路' : gatewayName(state.targetGatewayId)}
            </Descriptions.Item>
            <Descriptions.Item label="实际连接">
              {state.activeGatewayId !== null && state.activeGatewayId === state.confirmedGatewayId
                ? gatewayName(state.activeGatewayId)
                : state.supported ? '等待连接确认' : '—'}
            </Descriptions.Item>
            <Descriptions.Item label="切换状态">{switchStatus(state, target)}</Descriptions.Item>
          </Descriptions>
          {revisionChanged && (
            <Alert
              type="warning"
              showIcon
              message="设备配置已被更新"
              description="重新载入最新配置后再编辑。"
              action={<Button size="small" onClick={() => setDraft(null)}>重新载入配置</Button>}
              style={{ marginBottom: 16 }}
            />
          )}
          <Form layout="vertical" disabled={!state.supported || save.isPending || revisionChanged}>
            <Form.Item
              label="已授权的额外网关"
              extra={defaultGateway && `默认网关「${defaultGateway.name}」可直接连接。`}
            >
              <Select
                aria-label="已授权的额外网关"
                mode="multiple"
                allowClear
                placeholder="选择允许此设备连接的额外网关"
                options={gateways
                  .filter((gateway) => !gateway.isDefault || form.grantedGatewayIds.includes(gateway.id))
                  .map(option)}
                value={form.grantedGatewayIds}
                onChange={(grantedGatewayIds: string[]) => {
                  const targetGatewayId = form.targetGatewayId;
                  edit({
                    grantedGatewayIds,
                    targetGatewayId: targetGatewayId !== null
                      && (targetGatewayId === defaultGateway?.id || grantedGatewayIds.includes(targetGatewayId))
                      ? targetGatewayId
                      : null,
                  });
                }}
              />
            </Form.Item>
            <Form.Item label="指定网关" extra="在线设备在下次成功心跳后应用设置。">
              <Select<string>
                aria-label="指定网关"
                options={[
                  { value: '', label: '自动选路' },
                  ...gateways
                    .filter((gateway) => gateway.isDefault || form.grantedGatewayIds.includes(gateway.id))
                    .map(option),
                ]}
                value={form.targetGatewayId ?? ''}
                onChange={(targetGatewayId) => edit({ targetGatewayId: targetGatewayId || null })}
              />
            </Form.Item>
          </Form>
        </>
      )}
    </Modal>
  );
}
