# KooixMail Frontend Design

## 设计目标

把旧的 remote client 重写成只面对本地 Rust API 的 React SPA，同时保留临时邮箱产品的核心交互。

## 方案选择

### 1. 单页结构

- 选择单页 inbox，而不是多路由站点
- 理由：当前重写重点在核心邮箱闭环，不在内容站点

### 2. 本地 session 持久化

- 使用 `localStorage` 保存多个 mailbox token
- 理由：临时邮箱天然是轻状态场景，不需要更重的状态库

### 3. SSE + 手动刷新并存

- 实时更新靠 `EventSource`
- 同时保留手动刷新按钮，作为连接断开时的兜底链路

## 关键决策

### 取消 provider 抽象

- 不再保留 `KooixMail/Mail.tm/custom provider`
- 域名只来自本地 `/api/v1/domains`

### 取消 API Key 配置

- 旧 API Key 与私有域名逻辑属于 remote 平台能力
- 新版本只保留本地 mailbox token

### 不渲染 HTML 邮件

- 当前只展示原始 HTML 片段
- 理由：避免在无消毒链路下直接引入 XSS 风险

## 安全边界

- 不使用 `dangerouslySetInnerHTML`
- SSE token 暂通过 query 透传，适用于本地开发
- 公网部署前需进一步收紧 event 鉴权与 CORS

## 已知限制

- 还未集成国际化
- 没有附件下载 UI
- 没有真实 SMTP 健康面板

## 变更历史

### 2026-03-20 - React/Vite 本地 inbox 重写

**变更内容**: 新增创建邮箱、登录、会话切换、消息列表、详情与测试投递 UI

**变更理由**: 剥离 remote provider 与代理架构，回归纯临时邮箱服务前端

**影响范围**: 入口页面、交互模型、API 契约与运行方式

**决策依据**: 用最少依赖构成可运行 SPA，优先验证本地邮箱闭环
