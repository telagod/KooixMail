# KooixMail Frontend

## 模块定位

React 单页前端，直接消费本地 Rust API，提供临时邮箱创建、登录、收件箱浏览与测试投递入口。

## 为什么存在

旧前端深度绑定 remote provider、API Key 与 Mercure。此模块负责把体验收口到单一的本地邮箱服务。

## 核心职责

- 创建 mailbox 并持久化本地 session
- 切换多个已保存邮箱
- 展示消息列表与详情
- 订阅 SSE，实时刷新 inbox
- 提供 inbound 测试投递面板

## 不负责

- 服务端鉴权决策
- SMTP ingress
- 富文本邮件的安全渲染

## 依赖关系

- 依赖 `React 19`、`Vite`
- 依赖 `backend/` 暴露的 `/api/v1` 接口

## 快速启动

```bash
npm install
npm run dev
```

如需改后端地址，可设置：

```bash
VITE_API_BASE_URL=http://127.0.0.1:3000/api/v1
```
