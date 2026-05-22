# vpn

面向中小型企业（20–200 人）的轻量级自托管异地组网 VPN — 5 分钟搭建好的企业 VPN，员工无感连接。

**项目状态：开发中（Story 1.1 — 项目初始化）**

## 路线图

- 📋 完整规划文档：见 [`_bmad-output/planning-artifacts/`](_bmad-output/planning-artifacts/)
  - PRD：49 项 MVP 功能需求
  - UX 设计规范
  - 技术架构
  - Epic 与 Story（69 个 Story / 6 个 Epic）

## 技术栈

- **后端**：Rust + tokio + axum + sqlx + boringtun（WireGuard userspace）
- **客户端**：Rust + tun-rs（跨平台 TUN）+ Unix Socket/Named Pipe IPC
- **前端**：React 18 + TypeScript + Vite + Ant Design Pro
- **部署**：Docker 单容器 + 自动 HTTPS（ACME）

## 开发命令

需先安装 [just](https://github.com/casey/just)：

```bash
just              # 显示所有命令
just dev-server   # 启动后端开发服务
just dev-frontend # 启动前端开发服务
just test         # 运行所有测试
just build        # 构建 release 二进制 + 前端
```

详细的部署与使用文档将在 Story 6.1 完成。

## License

MIT
