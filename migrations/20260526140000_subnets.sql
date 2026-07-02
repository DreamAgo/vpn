-- 网段目录:集中维护"命名网段"(名称 + CIDR),供用户组路由 / 服务端 LAN / 节点路由等
-- 处直接下拉选择(各处仍按 CIDR 字符串存储,本表只作可选项目录)。

CREATE TABLE subnets (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    cidr TEXT NOT NULL,             -- 归一化后的 IPv4 CIDR,如 172.31.100.0/24
    created_at INTEGER NOT NULL,    -- unix ms
    updated_at INTEGER NOT NULL
);

CREATE UNIQUE INDEX idx_subnets_name ON subnets(name);
CREATE UNIQUE INDEX idx_subnets_cidr ON subnets(cidr);
