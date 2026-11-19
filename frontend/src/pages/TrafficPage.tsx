import { useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import {
  Alert, Button, Card, Col, Empty, Input, Row, Segmented, Skeleton, Space,
  Table, Tag, Tooltip, Typography, type TableColumnsType,
} from 'antd';
import { ArrowDownOutlined, ArrowUpOutlined, ReloadOutlined } from '@ant-design/icons';
import { formatTraffic, trafficApi, type PeerTraffic, type TrafficPoint } from '@/services/traffic';
import './TrafficPage.css';

const { Title, Text } = Typography;
const UPLOAD_COLOR = '#3b82f6';
const DOWNLOAD_COLOR = '#14b8a6';

function timestamp(value: number, detailed = false) {
  return new Intl.DateTimeFormat('zh-CN', {
    timeZone: 'Asia/Shanghai', month: '2-digit', day: '2-digit',
    ...(detailed ? { hour: '2-digit', minute: '2-digit', second: '2-digit' } : {}),
  }).format(value);
}

function TrafficChart({ points, hourly }: { points: TrafficPoint[]; hourly: boolean }) {
  const ceiling = Math.max(1, ...points.map((p) => Math.max(p.uploadBytes, p.downloadBytes)));
  const plot = (kind: 'uploadBytes' | 'downloadBytes') => points.map((p, i) =>
    `${60 + i * 890 / Math.max(1, points.length - 1)},${205 - p[kind] / ceiling * 170}`
  ).join(' ');
  return (
    <div className="traffic-chart">
      <svg viewBox="0 0 980 250" role="img" aria-label={hourly ? '最近24小时上传和下载流量趋势' : '每日上传和下载流量趋势'}>
        {[0, 0.5, 1].map((fraction) => (
          <g key={fraction}>
            <line x1={60} x2={950} y1={205 - fraction * 170} y2={205 - fraction * 170} className="traffic-grid" />
            <text x={52} y={209 - fraction * 170} textAnchor="end" className="traffic-axis">{formatTraffic(ceiling * fraction)}</text>
          </g>
        ))}
        <polyline fill="none" stroke={UPLOAD_COLOR} strokeWidth={2.5} strokeLinejoin="round" points={plot('uploadBytes')} />
        <polyline fill="none" stroke={DOWNLOAD_COLOR} strokeWidth={2.5} strokeLinejoin="round" points={plot('downloadBytes')} />
        {points.map((p, i) => (
          <g key={p.timestamp}>
            <circle cx={60 + i * 890 / Math.max(1, points.length - 1)} cy={205 - p.uploadBytes / ceiling * 170} r={3} fill={UPLOAD_COLOR}>
              <title>{timestamp(p.timestamp, true)} 上传 {formatTraffic(p.uploadBytes)}，下载 {formatTraffic(p.downloadBytes)}</title>
            </circle>
            <circle cx={60 + i * 890 / Math.max(1, points.length - 1)} cy={205 - p.downloadBytes / ceiling * 170} r={3} fill={DOWNLOAD_COLOR}>
              <title>{timestamp(p.timestamp, true)} 下载 {formatTraffic(p.downloadBytes)}</title>
            </circle>
            {(i === 0 || i === points.length - 1 || i % Math.ceil(points.length / 6) === 0) && (
              <text x={60 + i * 890 / Math.max(1, points.length - 1)} y={237} textAnchor="middle" className="traffic-axis">
                {hourly
                  ? new Intl.DateTimeFormat('zh-CN', { timeZone: 'Asia/Shanghai', hour: '2-digit', minute: '2-digit', hour12: false }).format(p.timestamp)
                  : timestamp(p.timestamp)}
              </text>
            )}
          </g>
        ))}
      </svg>
    </div>
  );
}

export function TrafficPage() {
  const [days, setDays] = useState(1);
  const [search, setSearch] = useState('');
  const query = useQuery({
    queryKey: ['traffic', days],
    queryFn: () => trafficApi.dashboard(days),
    refetchInterval: 5_000,
  });
  const data = query.data;
  const columns: TableColumnsType<PeerTraffic> = [
    { title: '终端', dataIndex: 'deviceName', render: (name, p) => <Space direction="vertical" size={0}><Text strong>{name}</Text><Text type="secondary">{p.username} · {p.vpnIp}</Text></Space> },
    { title: '上传流量', dataIndex: 'uploadBytes', sorter: (a, b) => a.uploadBytes - b.uploadBytes, render: (v: number) => formatTraffic(v) },
    { title: '下载流量', dataIndex: 'downloadBytes', sorter: (a, b) => a.downloadBytes - b.downloadBytes, render: (v: number) => formatTraffic(v) },
    { title: '上传速率', dataIndex: 'uploadBps', render: (v: number | null) => formatTraffic(v, true) },
    { title: '下载速率', dataIndex: 'downloadBps', render: (v: number | null) => formatTraffic(v, true) },
    { title: '最近采集', dataIndex: 'sampledAt', render: (v: number) => timestamp(v, true) },
  ];
  const stats = [
    { label: '今日流量', value: data ? formatTraffic(data.today.uploadBytes + data.today.downloadBytes) : '—', note: '北京时间 00:00 起' },
    { label: '累计流量', value: data ? formatTraffic(data.total.uploadBytes + data.total.downloadBytes) : '—', note: data?.firstSampleAt ? `开始于 ${timestamp(data.firstSampleAt, true)}` : '等待首次采集' },
    { label: '实时上传', value: formatTraffic(data?.uploadBps, true), note: '客户端 → 网关', icon: <ArrowUpOutlined /> },
    { label: '实时下载', value: formatTraffic(data?.downloadBps, true), note: '网关 → 客户端', icon: <ArrowDownOutlined /> },
  ];
  return (
    <div className="traffic-page">
      <div className="page-heading">
        <div><Title level={4} style={{ margin: 0 }}>流量统计</Title><Text type="secondary">VPN 终端用量与传输趋势</Text></div>
        <Space wrap>
          <Segmented value={days} onChange={(value) => setDays(Number(value))} options={[{ label: '24 小时', value: 1 }, { label: '7 天', value: 7 }, { label: '30 天', value: 30 }]} />
          <Button icon={<ReloadOutlined />} loading={query.isFetching} onClick={() => query.refetch()}>刷新</Button>
        </Space>
      </div>
      {query.isError && <Alert type="error" showIcon message="流量统计读取失败" description={query.error instanceof Error ? query.error.message : '请稍后重试'} />}
      {query.isPending ? <Skeleton active /> : <>
        <Row gutter={[16, 16]}>{stats.map((s) => <Col key={s.label} xs={24} sm={12} xl={6}>
          <Card className="traffic-metric"><Text type="secondary">{s.icon} {s.label}</Text><div className="traffic-value">{s.value}</div><Text type="secondary">{s.note}</Text></Card>
        </Col>)}</Row>
        <div className="traffic-sources">{data?.sources.map((s) => {
          const fresh = s.sampledAt != null && data.to - s.sampledAt <= 20_000 && data.to >= s.sampledAt;
          return <Tooltip key={s.id} title={s.sampledAt ? `最近采集 ${timestamp(s.sampledAt, true)}` : '需要支持流量采集的 WireGuard 后端或新版远程网关'}>
            <Tag color={fresh ? 'success' : 'default'}>{s.name} · {fresh ? '采集中' : s.sampledAt ? '采集中断' : '等待数据'}</Tag>
          </Tooltip>;
        })}</div>
        <Card title="流量趋势" extra={<Space><Text style={{ color: UPLOAD_COLOR }}>↑ 上传 {formatTraffic(data?.period.uploadBytes)}</Text><Text style={{ color: DOWNLOAD_COLOR }}>↓ 下载 {formatTraffic(data?.period.downloadBytes)}</Text></Space>}>
          {data?.firstSampleAt ? <TrafficChart points={data.series} hourly={days === 1} /> : <Empty description="等待网关采集流量数据" />}
        </Card>
        <Card title="终端流量" extra={<Input.Search allowClear placeholder="搜索终端、账号或 IP" aria-label="搜索终端流量" onChange={(e) => setSearch(e.target.value)} style={{ maxWidth: 280 }} />}>
          <Table<PeerTraffic> rowKey="peerId" columns={columns}
            dataSource={data?.peers.filter((p) => `${p.deviceName} ${p.username} ${p.vpnIp}`.toLocaleLowerCase().includes(search.toLocaleLowerCase()))}
            pagination={{ defaultPageSize: 10, showSizeChanger: true }} scroll={{ x: 900 }} />
        </Card>
        <Text type="secondary">每 5 秒采集一次，以客户端视角统计 WireGuard 传输字节。首次采集建立基线，趋势按小时或北京时间日期汇总，历史从启用统计后开始记录。</Text>
      </>}
    </div>
  );
}
