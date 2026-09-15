/**
 * 网段组管理页。
 *
 * 集中维护"命名网段"(名称 + CIDR);在用户组路由 / 服务端 LAN / 节点路由等处可直接下拉选择。
 */
import { useEffect, useState } from 'react';
import {
  Button,
  Tag,
  Space,
  Popconfirm,
  App,
  Typography,
  Table,
  Modal,
  Form,
  Input,
} from 'antd';
import { PlusOutlined } from '@ant-design/icons';
import { useQuery } from '@tanstack/react-query';
import dayjs from 'dayjs';

import { subnetsApi } from '@/services/subnets';
import { ApiError } from '@/services/http';
import { ErrorCodes } from '@/types/api';
import type { SubnetDto } from '@/types/api';
import { isValidCidr, parseCidrText } from '@/utils/cidr';

const { Title, Text } = Typography;

function describeError(err: unknown, fallback: string): string {
  if (err instanceof ApiError) {
    if (err.code === ErrorCodes.DuplicateResource) return '网段组名称已存在';
    if (err.code === ErrorCodes.NoAccess || err.code === ErrorCodes.RequireAdmin)
      return '无权限执行该操作';
    return err.message || fallback;
  }
  return fallback;
}

interface FormValues {
  name: string;
  cidr: string;
}

export function SubnetsPage() {
  const { message } = App.useApp();
  const [editing, setEditing] = useState<SubnetDto | null>(null);
  const [modalOpen, setModalOpen] = useState(false);
  const [saving, setSaving] = useState(false);
  const [form] = Form.useForm<FormValues>();

  const { data: subnets, isLoading, refetch } = useQuery({
    queryKey: ['subnets'],
    queryFn: subnetsApi.listSubnets,
  });

  useEffect(() => {
    if (modalOpen) {
      form.setFieldsValue({ name: editing?.name ?? '', cidr: editing ? (editing.cidrs ?? parseCidrText(editing.cidr)).join('\n') : '' });
    }
  }, [modalOpen, editing, form]);

  const openCreate = () => {
    setEditing(null);
    setModalOpen(true);
  };
  const openEdit = (s: SubnetDto) => {
    setEditing(s);
    setModalOpen(true);
  };

  const handleSave = async (values: FormValues) => {
    setSaving(true);
    try {
      const name = values.name.trim();
      const cidrs = parseCidrText(values.cidr);
      if (editing) {
        await subnetsApi.updateSubnet(editing.id, { name, cidrs });
      } else {
        await subnetsApi.createSubnet({ name, cidrs });
      }
      message.success(editing ? '网段组已更新' : '网段组已新增');
      setModalOpen(false);
      refetch();
    } catch (err) {
      message.error(describeError(err, '保存失败'));
    } finally {
      setSaving(false);
    }
  };

  const handleDelete = async (s: SubnetDto) => {
    try {
      await subnetsApi.deleteSubnet(s.id);
      message.success('已删除网段组');
      refetch();
    } catch (err) {
      message.error(describeError(err, '删除失败'));
    }
  };

  const columns = [
    { title: '名称', dataIndex: 'name', key: 'name', render: (n: string) => <Text strong>{n}</Text> },
    {
      title: '网段（CIDR）',
      dataIndex: 'cidr',
      key: 'cidr',
      render: (_: string, record: SubnetDto) => (
        <Space wrap size={[0, 4]}>{(record.cidrs ?? parseCidrText(record.cidr)).map((c) => <Tag key={c} color="blue">{c}</Tag>)}</Space>
      ),
    },
    {
      title: '路由使用',
      dataIndex: 'usageCount',
      key: 'usageCount',
      width: 80,
      render: (n: number) =>
        n > 0 ? (
          <Tag color="gold">{n} 处</Tag>
        ) : (
          <Text type="secondary">未使用</Text>
        ),
    },
    {
      title: '创建时间',
      dataIndex: 'createdAt',
      key: 'createdAt',
      width: 170,
      render: (t: number) => <Text type="secondary">{dayjs(t).format('YYYY-MM-DD HH:mm')}</Text>,
    },
    {
      title: '操作',
      key: 'action',
      width: 140,
      render: (_: unknown, record: SubnetDto) => (
        <Space size="small">
          <Button type="link" size="small" onClick={() => openEdit(record)}>
            编辑
          </Button>
          <Popconfirm
            title="删除网段组"
            description={
              record.usageCount > 0
                ? `该网段已被 ${record.usageCount} 处引用（用户组/节点/服务端路由）。删除仅从可选目录移除，已保存的网段值不受影响。`
                : '删除后不影响已保存到各处的网段值，仅从可选目录移除。'
            }
            okText="删除"
            okButtonProps={{ danger: true }}
            cancelText="取消"
            onConfirm={() => handleDelete(record)}
          >
            <Button type="link" size="small" danger>
              删除
            </Button>
          </Popconfirm>
        </Space>
      ),
    },
  ];

  return (
    <div>
      <Space
        wrap
        className="page-heading"
        align="center"
        style={{ width: '100%', justifyContent: 'space-between', marginBottom: 16 }}
      >
        <Title level={4} style={{ margin: 0 }}>
          网段组管理
          <Text type="secondary" style={{ fontSize: 13, marginLeft: 12, fontWeight: 400 }}>
            每组包含多个 CIDR，供用户组 / 服务端 / 节点路由一次选择
          </Text>
        </Title>
        <Button type="primary" icon={<PlusOutlined />} onClick={openCreate}>
          新增网段组
        </Button>
      </Space>

      <Table<SubnetDto>
        scroll={{ x: 760 }}
        rowKey="id"
        loading={isLoading}
        columns={columns}
        dataSource={subnets ?? []}
        pagination={false}
      />

      <Modal
        open={modalOpen}
        title={editing ? '编辑网段组' : '新增网段组'}
        onCancel={() => !saving && setModalOpen(false)}
        onOk={() => form.submit()}
        confirmLoading={saving}
        okText="保存"
        cancelText="取消"
        destroyOnClose
      >
        <Form<FormValues> form={form} layout="vertical" preserve={false} onFinish={handleSave}>
          <Form.Item
            name="name"
            label="名称"
            rules={[
              { required: true, message: '请输入名称' },
              { whitespace: true, message: '名称不能为空白' },
            ]}
          >
            <Input placeholder="如：办公网 / Docker 网络 / 数据中心" maxLength={64} />
          </Form.Item>
          <Form.Item
            name="cidr"
            label="网段（CIDR）"
            rules={[
              { required: true, message: '请输入 CIDR' },
              {
                validator: (_r, v: string) =>
                  !v || (parseCidrText(v).length > 0 && parseCidrText(v).every(isValidCidr))
                    ? Promise.resolve()
                    : Promise.reject(new Error(`请每行输入一个合法 CIDR：${parseCidrText(v).filter((c) => !isValidCidr(c)).join('、') || '不能为空'}`)),
              },
            ]}
            extra="每行一个 CIDR，也支持空格或逗号分隔；保存时归一化并去重。编辑此组不会自动修改已保存的路由。"
          >
            <Input.TextArea autoSize={{ minRows: 7, maxRows: 14 }} placeholder={'135.10.0.0/16\n10.196.184.0/24\n10.196.163.126/32'} />
          </Form.Item>
        </Form>
      </Modal>
    </div>
  );
}
