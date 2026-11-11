import { App, Button, Modal, Popconfirm, Space, Table, Tag, Typography } from 'antd';
import { useMutation, useQueryClient } from '@tanstack/react-query';

import { clusterApi, clusterGatewaysQueryKey, type ClusterGateway } from '@/services/cluster';
import { ApiError } from '@/services/http';

const { Text } = Typography;

interface Props {
  gateways: ClusterGateway[];
  onClose: () => void;
}

export function ClusterGatewaysModal({ gateways, onClose }: Props) {
  const { message } = App.useApp();
  const queryClient = useQueryClient();
  const setDefault = useMutation({
    mutationFn: clusterApi.setDefaultGateway,
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: clusterGatewaysQueryKey });
      await queryClient.invalidateQueries({ queryKey: ['cluster-device'] });
      message.success('默认网关已更新');
    },
    onError: (err) => message.error(err instanceof ApiError ? err.message : '更新默认网关失败'),
  });

  const close = () => {
    if (!setDefault.isPending) onClose();
  };

  return (
    <Modal
      open
      title="集群网关"
      width={720}
      onCancel={close}
      maskClosable={false}
      footer={<Button disabled={setDefault.isPending} onClick={close}>关闭</Button>}
    >
      <Table<ClusterGateway>
        rowKey="id"
        size="small"
        dataSource={gateways}
        pagination={gateways.length > 8 ? { pageSize: 8, showSizeChanger: false } : false}
        scroll={{ x: 590 }}
        locale={{ emptyText: '暂无集群网关' }}
        columns={[
          {
            title: '网关',
            dataIndex: 'name',
            render: (_, gateway) => (
              <Space size={6} wrap>
                <span>{gateway.name}</span>
                {gateway.isDefault && <Tag color="blue">默认</Tag>}
              </Space>
            ),
          },
          {
            title: '探测地址',
            dataIndex: 'probeEndpoint',
            render: (endpoint) => endpoint ? <Text code>{endpoint}</Text> : '—',
          },
          {
            title: '状态',
            width: 95,
            render: (_, gateway) => gateway.ready ? <Tag color="green">就绪</Tag> : <Tag>未就绪</Tag>,
          },
          {
            title: '操作',
            width: 112,
            render: (_, gateway) => gateway.isDefault ? null : (
              <Popconfirm
                title={`将「${gateway.name}」设为默认网关？`}
                description="有 VPN 使用权限的设备可直接使用新的默认网关。"
                okText="设为默认"
                cancelText="取消"
                onConfirm={() => setDefault.mutateAsync(gateway.id)}
              >
                <Button
                  type="link"
                  size="small"
                  disabled={setDefault.isPending || !gateway.ready}
                >
                  设为默认
                </Button>
              </Popconfirm>
            ),
          },
        ]}
      />
    </Modal>
  );
}
