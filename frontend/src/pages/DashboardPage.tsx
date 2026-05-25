/**
 * Story 2.12：仪表盘骨架 + 系统信息卡片。
 *
 * 通过 GET /admin/system/info 拉取服务端信息并展示。
 * 节点/流量等统计卡片暂为占位（Story 4.x / 5.x 填充真实数据）。
 */
import { Card, Col, Row, Statistic, Descriptions, Tag, Skeleton, Alert, Typography } from 'antd';
import { ApiOutlined, ClusterOutlined } from '@ant-design/icons';
import { useQuery } from '@tanstack/react-query';
import dayjs from 'dayjs';

import { systemApi } from '@/services/auth';

const { Title } = Typography;

export function DashboardPage() {
  const { data, isLoading, isError, error } = useQuery({
    queryKey: ['system-info'],
    queryFn: () => systemApi.getSystemInfo(),
  });

  return (
    <div style={{ padding: 24 }}>
      <Title level={4} style={{ marginBottom: 16 }}>
        仪表盘
      </Title>

      <Row gutter={[16, 16]} style={{ marginBottom: 16 }}>
        <Col xs={24} sm={12} lg={8}>
          <Card>
            <Statistic title="在线节点" value="—" prefix={<ApiOutlined />} />
          </Card>
        </Col>
        <Col xs={24} sm={12} lg={8}>
          <Card>
            <Statistic title="注册用户" value="—" prefix={<ClusterOutlined />} />
          </Card>
        </Col>
        <Col xs={24} sm={12} lg={8}>
          <Card>
            <Statistic title="VPN 子网" value={data?.vpnSubnet ?? '—'} />
          </Card>
        </Col>
      </Row>

      <Card title="系统信息">
        {isError && (
          <Alert
            type="error"
            showIcon
            message="无法加载系统信息"
            description={error instanceof Error ? error.message : '未知错误'}
          />
        )}
        {isLoading && <Skeleton active paragraph={{ rows: 4 }} />}
        {data && (
          <Descriptions column={{ xs: 1, sm: 2 }} bordered size="small">
            <Descriptions.Item label="版本">
              <Tag color="blue">v{data.version}</Tag>
            </Descriptions.Item>
            <Descriptions.Item label="监听端口">{data.listenPort}</Descriptions.Item>
            <Descriptions.Item label="VPN 子网">{data.vpnSubnet}</Descriptions.Item>
            <Descriptions.Item label="服务端 Endpoint">
              {data.serverEndpoint}
            </Descriptions.Item>
            <Descriptions.Item label="服务端公钥" span={2}>
              <Typography.Text code copyable={{ text: data.serverPublicKey }}>
                {data.serverPublicKey}
              </Typography.Text>
            </Descriptions.Item>
            <Descriptions.Item label="启动时间" span={2}>
              {dayjs(data.startedAt).format('YYYY-MM-DD HH:mm:ss')}
            </Descriptions.Item>
          </Descriptions>
        )}
      </Card>
    </div>
  );
}
