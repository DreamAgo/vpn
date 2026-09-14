import { useRef } from 'react';
import { App, Button } from 'antd';
import { ReloadOutlined } from '@ant-design/icons';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { systemApi } from '@/services/auth';

/** 挂载在公共布局中，切换页面不会中断重启状态检查。 */
export function RestartServerButton() {
  const { modal, message } = App.useApp();
  const queryClient = useQueryClient();
  const confirming = useRef(false);
  const restart = useMutation({
    mutationKey: ['restart-server'],
    retry: false,
    mutationFn: async () => {
      await systemApi.restartServer();
      message.info('重启请求已接受，正在等待服务恢复…');
      const deadline = Date.now() + 90_000;
      while (Date.now() < deadline) {
        await new Promise((resolve) => setTimeout(resolve, 3000));
        try {
          const [network, integration] = await Promise.all([
            systemApi.getNetworkSettings(),
            systemApi.getIntegrationSettings(),
          ]);
          if (!network.restartRequired && !integration.restartRequired) {
            return { network, integration };
          }
        } catch {
          // 重启期间连接可能中断，等待下一次检查。
        }
      }
      throw new Error('暂未确认服务恢复或配置生效，请稍后刷新页面检查；不要重复提交重启');
    },
    onSuccess: ({ network, integration }) => {
      queryClient.setQueryData(['network-settings'], network);
      queryClient.setQueryData(['integration-settings'], integration);
      void queryClient.invalidateQueries({ queryKey: ['system-info'] });
      message.success('服务已可访问，已保存的网络与集成配置已生效');
    },
    onError: (error) => message.error(error instanceof Error ? error.message : '重启请求失败，请检查服务状态'),
  });

  const confirm = () => {
    if (confirming.current || restart.isPending) return;
    confirming.current = true;
    modal.confirm({
      title: '重启服务端？',
      content: '在线 VPN 连接会暂时中断。重启会应用所有已保存的配置，请先保存当前页面的修改；未保存的内容不会被应用。',
      okText: '重启服务端',
      okButtonProps: { danger: true },
      cancelText: '取消',
      onOk: () => { restart.mutate(); },
      afterClose: () => { confirming.current = false; },
    });
  };

  return (
    <Button icon={<ReloadOutlined />} loading={restart.isPending} onClick={confirm}>
      {restart.isPending ? '等待服务恢复' : '重启服务端'}
    </Button>
  );
}
