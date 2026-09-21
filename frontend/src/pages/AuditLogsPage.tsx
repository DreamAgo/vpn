import { useMemo, useRef, useState } from 'react';
import { Alert, App, Button, DatePicker, Descriptions, Input, Select, Space, Table, Tag, Typography } from 'antd';
import { ProTable, type ActionType, type ProColumns } from '@ant-design/pro-components';
import { useSearchParams } from 'react-router-dom';
import dayjs from 'dayjs';
import { auditApi, auditActionLabels as labels } from '@/services/audit';
import type { AuditLogDto, AuditLogQuery } from '@/types/api';


function metadata(record: AuditLogDto): Record<string, unknown> {
  try { const value = JSON.parse(record.metadata ?? '{}'); return value && typeof value === 'object' && !Array.isArray(value) ? value : {}; }
  catch { return {}; }
}
function outcome(record: AuditLogDto): string {
  const data = metadata(record);
  if (data.outcome === 'committed') return '已提交';
  if (data.outcome === 'failed' || (record.statusCode ?? 0) >= 400 || record.action.endsWith('_failed')) return '失败';
  if (data.outcome === 'success' || (record.statusCode != null && record.statusCode >= 200 && record.statusCode < 400) || record.action.endsWith('_success')) return '成功';
  return '历史记录未提供';
}
const display = (value: unknown) => value === undefined ? '—' : JSON.stringify(value);
function Details({ record }: { record: AuditLogDto }) {
  const data = metadata(record);
  const changes = data.changes && typeof data.changes === 'object' && !Array.isArray(data.changes)
    ? Object.entries(data.changes).map(([field, value]) => ({field, ...(value && typeof value === 'object' ? value : {})})) as {field:string;before?:unknown;after?:unknown;changed?:boolean}[] : [];
  return <Space direction="vertical" style={{width:'100%'}}>
    <Descriptions size="small" column={2} items={[
      {key:'actor',label:'操作者 ID',children:record.userId ?? '未提供'},
      {key:'ip',label:'来源 IP',children:record.ipAddr ?? '未提供'},
      {key:'request',label:'请求 ID',children:String(data.request_id ?? '历史记录未提供')},
      {key:'status',label:'HTTP 状态',children:record.statusCode ?? '未提供'},
      {key:'agent',label:'客户端',children:record.userAgent ?? '未提供'},
      {key:'reason',label:'结果说明',children:String(data.reason ?? data.reason_code ?? (data.outcome === 'committed' ? '数据库变更已提交；运行时应用失败会另记失败记录' : '—'))},
    ]}/>
    {changes.length > 0 ? <Table size="small" pagination={false} rowKey="field" dataSource={changes} columns={[
      {title:'变更字段',dataIndex:'field'},
      {title:'变更前',render:(_,r)=>r.changed ? '敏感值不记录' : display(r.before)},
      {title:'变更后',render:(_,r)=>r.changed ? '已变更' : display(r.after)},
    ]}/> : <Space direction="vertical"><Typography.Text type="secondary">此记录未提供字段差异。</Typography.Text>
      {record.action === 'grant.expiry.update' && data.before !== undefined && <pre>{JSON.stringify({before:data.before,expires_at:data.expires_at},null,2)}</pre>}</Space>}
  </Space>;
}
// Export only an explicit safe column set; raw metadata and HTTP headers are never exported.
function csvCell(value: unknown): string {
  let text = String(value ?? '');
  if (/^[\s]*[=+@-]/.test(text)) text = `'${text}`;
  return `"${text.replaceAll('"','""')}"`;
}

export function AuditLogsPage() {
  const { message } = App.useApp();
  const [params,setParams] = useSearchParams();
  const actionRef = useRef<ActionType>(null);
  const [healthWarning,setHealthWarning] = useState<string>();
  const [exporting,setExporting] = useState(false);
  const [defaults] = useState(()=>({from:dayjs().subtract(7,'day').startOf('day').valueOf(),to:dayjs().endOf('day').valueOf()}));
  const query: AuditLogQuery = useMemo(()=>({
    from:Number(params.get('from')) || defaults.from, to:Number(params.get('to')) || defaults.to,
    action:params.get('action') || undefined, username:params.get('username') || undefined,
    userId:params.get('userId') || undefined, resource:params.get('resource') || undefined,
    outcome:params.get('outcome') || undefined, category:params.get('category') || undefined,
  }),[params,defaults]);
  const filter = (values: Record<string,string|undefined>) => {
    const next = new URLSearchParams(params);
    for (const [key,value] of Object.entries(values)) {if(value) next.set(key,value);else next.delete(key);}
    setParams(next,{replace:true});
  };
  const columns: ProColumns<AuditLogDto>[] = [
    {title:'时间',dataIndex:'createdAt',width:180,render:(_,r)=>dayjs(r.createdAt).format('YYYY-MM-DD HH:mm:ss')},
    {title:'操作者',dataIndex:'username',render:(_,r)=>r.username ?? r.userId ?? '未识别'},
    {title:'操作',dataIndex:'action',render:(_,r)=><span title={r.action}>{labels[r.action] ?? r.action}</span>},
    {title:'目标',dataIndex:'resource',ellipsis:true},
    {title:'来源 IP',dataIndex:'ipAddr'},
    {title:'结果',render:(_,r)=><Tag color={outcome(r)==='失败'?'error':outcome(r)==='历史记录未提供'?'default':'success'}>{outcome(r)}</Tag>},
  ];
  const exportLogs = async () => {
    setExporting(true);
    try {
      const rows: AuditLogDto[]=[];
      const frozen = {...query,to:Math.min(query.to ?? Date.now(),Date.now())};
      for(let page=1;page<=100;page++) {
        const data=await auditApi.listAuditLogs({...frozen,page,pageSize:100});
        if(data.total>10000) throw new Error('结果超过 10000 条，请缩小筛选范围后导出');
        rows.push(...data.items);
        if(rows.length>=data.total || !data.items.length) break;
      }
      const lines=[['时间','操作者','操作者ID','操作','目标','来源IP','结果','HTTP状态','请求ID'],...rows.map(r=>[
        dayjs(r.createdAt).format('YYYY-MM-DD HH:mm:ss'),r.username,r.userId,labels[r.action] ?? r.action,r.resource,r.ipAddr,outcome(r),r.statusCode,metadata(r).request_id,
      ])];
      const url=URL.createObjectURL(new Blob(['\uFEFF'+lines.map(row=>row.map(csvCell).join(',')).join('\r\n')],{type:'text/csv;charset=utf-8'}));
      const link=document.createElement('a');link.href=url;link.download='audit-logs.csv';link.click();
      setTimeout(()=>URL.revokeObjectURL(url),1000);
    } catch(error) {message.error(error instanceof Error?error.message:'导出失败');}
    finally {setExporting(false);}
  };
  return <div>
    <Typography.Title level={4}>审计日志</Typography.Title>
    {healthWarning && <Alert type="warning" showIcon message={healthWarning} style={{marginBottom:12}}/>}
    <ProTable<AuditLogDto,AuditLogQuery> rowKey="id" columns={columns} actionRef={actionRef} params={query}
      search={false} scroll={{x:1100}} pagination={{defaultPageSize:20,showSizeChanger:true}}
      expandable={{expandedRowRender:r=><Details record={r}/>}}
      toolBarRender={()=>[<Button key="export" loading={exporting} onClick={exportLogs}>导出筛选结果（脱敏 CSV）</Button>]}
      toolbar={{search:<Space wrap>
        <DatePicker.RangePicker showTime allowClear={false} value={[dayjs(query.from),dayjs(query.to)]}
          onChange={r=>r?.[0]&&r[1]&&filter({from:String(r[0].valueOf()),to:String(r[1].valueOf())})}/>
        <Select allowClear showSearch optionFilterProp="label" placeholder="操作" style={{width:180}} value={query.action}
          options={Object.entries(labels).map(([value,label])=>({value,label}))} onChange={value=>filter({action:value})}/>
        <Select allowClear placeholder="结果" style={{width:100}} value={query.outcome} options={[{value:'success',label:'成功/已提交'},{value:'failed',label:'失败'}]} onChange={value=>filter({outcome:value})}/>
        <Select allowClear placeholder="类别" style={{width:130}} value={query.category} options={[
          ['network','网络/DNS'],['user','用户'],['group','用户组'],['peer','节点'],['backup','备份'],['api_key','API 密钥'],['notification','通知'],['integration','集成'],['system','系统'],['login','登录'],
        ].map(([value,label])=>({value,label}))} onChange={value=>filter({category:value})}/>
        <Input.Search key={`name-${query.username ?? ''}`} allowClear placeholder="用户名 / 密钥名称" defaultValue={query.username} onSearch={value=>filter({username:value})} style={{width:190}}/>
        <Input.Search key={`id-${query.userId ?? ''}`} allowClear placeholder="操作者 ID" defaultValue={query.userId} onSearch={value=>filter({userId:value})} style={{width:170}}/>
        <Input.Search key={`resource-${query.resource ?? ''}`} allowClear placeholder="目标 ID / 路径" defaultValue={query.resource} onSearch={value=>filter({resource:value})} style={{width:190}}/>
      </Space>}}
      request={async p=>{try{
        try {const h=await auditApi.health();setHealthWarning(h.failedWrites || h.failedTransactionWrites || h.droppedEvents ? `本次运行：审计写入失败 ${h.failedWrites} 次，业务回滚 ${h.failedTransactionWrites} 次，过载丢弃 ${h.droppedEvents} 条，请检查服务端日志。` : undefined);} catch {setHealthWarning('无法读取审计健康状态');}
        const page=await auditApi.listAuditLogs({...query,page:p.current,pageSize:p.pageSize});return {data:page.items,total:page.total,success:true};}
        catch{message.error('加载审计日志失败');return {data:[],total:0,success:false};}}}/>
  </div>;
}
