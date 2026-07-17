---
title: '客户端与服务端版本号显示'
type: 'feature'
created: '2026-07-17'
status: 'done'
route: 'one-shot'
---

# 客户端与服务端版本号显示

## Intent

**Problem:** 客户端界面缺少直接可见的当前版本，服务端版本只能在仪表盘系统信息中查看，不便于快速确认部署版本。

**Approach:** 复用客户端 Tauri 诊断信息中的应用版本和服务端系统信息接口中的编译版本，在客户端品牌区、更新设置及管理后台侧栏中持续展示，并处理加载失败、重复前缀与长版本溢出。

## Suggested Review Order

**客户端版本展示**

- 从真实诊断元数据统一格式化版本，并覆盖登录前后与更新设置。
  [`App.tsx:52`](../../desktop/src/App.tsx#L52)

- 品牌区限制长版本宽度，避免挤压窗口控制区。
  [`styles.css:133`](../../desktop/src/styles.css#L133)

**服务端版本展示**

- 复用系统信息查询，在管理员侧栏展示加载、成功与失败状态。
  [`AppLayout.tsx:157`](../../frontend/src/components/layout/AppLayout.tsx#L157)

- 侧栏展开和折叠状态均提供语义标签与溢出保护。
  [`AppLayout.tsx:214`](../../frontend/src/components/layout/AppLayout.tsx#L214)
