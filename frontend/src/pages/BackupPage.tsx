import { useState } from 'react';
import {
  Alert,
  App,
  Button,
  Card,
  Descriptions,
  Space,
  Typography,
  Upload,
  type UploadFile,
} from 'antd';
import { DownloadOutlined, UploadOutlined } from '@ant-design/icons';
import { useNavigate } from 'react-router-dom';

import { backupApi, type RestoreResult } from '@/services/backup';
import { useAuthStore } from '@/stores/authStore';

const { Title, Text, Paragraph } = Typography;

export function BackupPage() {
  const { message, modal } = App.useApp();
  const navigate = useNavigate();
  const clearSession = useAuthStore((s) => s.clearSession);
  const [downloading, setDownloading] = useState(false);
  const [restoring, setRestoring] = useState(false);
  const [fileList, setFileList] = useState<UploadFile[]>([]);
  const [lastRestore, setLastRestore] = useState<RestoreResult | null>(null);

  const download = async () => {
    setDownloading(true);
    try {
      await backupApi.downloadBackup();
      message.success('备份文件已生成');
    } catch (err) {
      message.error(err instanceof Error ? err.message : '下载备份失败');
    } finally {
      setDownloading(false);
    }
  };

  const restore = async () => {
    const file = fileList[0]?.originFileObj;
    if (!file) {
      message.warning('请先选择备份文件');
      return;
    }

    modal.confirm({
      title: '确认恢复备份？',
      content:
        '恢复会覆盖当前用户、用户组、节点、网段、系统配置和审计日志，并清空所有登录会话。恢复完成后需要重启服务端让隧道运行态完全生效。',
      okText: '恢复备份',
      okButtonProps: { danger: true },
      cancelText: '取消',
      onOk: async () => {
        setRestoring(true);
        try {
          const text = await file.text();
          const archive = JSON.parse(text);
          const result = await backupApi.restoreBackup(archive);
          setLastRestore(result);
          message.success('备份已恢复，请重新登录');
          clearSession();
          navigate('/login', { replace: true });
        } catch (err) {
          message.error(err instanceof Error ? err.message : '恢复备份失败');
        } finally {
          setRestoring(false);
        }
      },
    });
  };

  return (
    <div style={{ maxWidth: 880 }}>
      <Title level={4} style={{ marginBottom: 16 }}>
        备份与恢复
      </Title>

      <Space direction="vertical" size={16} style={{ width: '100%' }}>
        <Alert
          type="info"
          showIcon
          message="备份范围"
          description="备份包含账号、密码哈希、用户组、网段目录、节点配置、站点网关、服务端系统配置和审计日志；不包含当前登录会话，恢复后所有用户需要重新登录。"
        />

        <Card title="创建备份">
          <Paragraph type="secondary">
            下载当前控制面配置的 JSON 备份文件。建议在新增节点、调整网段或升级服务前先保存一份。
          </Paragraph>
          <Button
            type="primary"
            icon={<DownloadOutlined />}
            loading={downloading}
            onClick={download}
          >
            下载备份
          </Button>
        </Card>

        <Card title="恢复备份">
          <Alert
            type="warning"
            showIcon
            style={{ marginBottom: 16 }}
            message="恢复会覆盖当前数据"
            description="恢复成功后需要重启服务端，使 WireGuard 密钥、节点和路由运行态重新加载。"
          />
          <Space direction="vertical" size={12} style={{ width: '100%' }}>
            <Upload
              accept="application/json,.json"
              maxCount={1}
              fileList={fileList}
              beforeUpload={() => false}
              onChange={({ fileList: next }) => setFileList(next)}
            >
              <Button icon={<UploadOutlined />}>选择备份文件</Button>
            </Upload>
            <Button danger loading={restoring} disabled={fileList.length === 0} onClick={restore}>
              恢复备份
            </Button>
          </Space>
        </Card>

        {lastRestore && (
          <Card title="最近恢复结果">
            <Descriptions column={2} size="small">
              <Descriptions.Item label="用户">{lastRestore.users}</Descriptions.Item>
              <Descriptions.Item label="节点">{lastRestore.peers}</Descriptions.Item>
              <Descriptions.Item label="用户组">{lastRestore.user_groups}</Descriptions.Item>
              <Descriptions.Item label="网段">{lastRestore.subnets}</Descriptions.Item>
              <Descriptions.Item label="审计日志">{lastRestore.audit_logs}</Descriptions.Item>
              <Descriptions.Item label="需要重启">
                {lastRestore.requires_restart ? <Text type="danger">是</Text> : '否'}
              </Descriptions.Item>
            </Descriptions>
          </Card>
        )}
      </Space>
    </div>
  );
}
