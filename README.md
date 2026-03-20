# KooixMail

自管临时邮箱服务 — Rust 后端 + React 前端，零第三方依赖。

## 特性

- 邮箱创建/登录/删除，Argon2 密码哈希
- 消息收发/分页/已读未读/删除
- SSE 实时推送（断线自动重连，指数退避）
- HTTP inbound endpoint + SMTP ingress（STARTTLS / implicit TLS）
- SPF / DKIM / DMARC 校验，本地域伪造保护
- Per-IP 速率限制，消息大小限制，Greylisting，RBL/DNSBL
- 液态玻璃美学 UI，深浅色主题切换（系统偏好跟随 + 持久化）
- 响应式布局（1320px / 960px / 480px 三断点）
- 无障碍：aria-live 通知、aria-label 卡片、键盘导航

## 快速启动

```bash
# 一键启动（需要 Rust + Node.js）
make dev

# 或分别启动
cd backend && cargo run        # http://127.0.0.1:3000
cd frontend && npm run dev     # http://127.0.0.1:5173
```

环境变量参考 [.env.example](.env.example)。

## 测试

```bash
make test

# 或分别测试
cd backend && cargo test       # 90 个测试
cd frontend && npx tsc --noEmit
```

测试覆盖：
- `auth.rs` — 地址规范化、密码校验、过期检查、token 提取、hash 往返（32 个）
- `db/` — 全部 CRUD + 级联删除 + 分页排序（16 个）
- `routes.rs` — 12 条 API 路由全覆盖，含认证/越权/冲突/ingress token（21 个）
- `inbound.rs` — 投递 + 速率限制 + 本地域防伪 + Greylist + trusted 跳过 + 超限拒绝（8 个）
- `smtp.rs` — TCP SMTP 对话 + STARTTLS 握手 + TLS 配置校验（7 个）
- `lib.rs` — 配置解析（5 个）

## 架构

```
backend/     Rust (axum + tokio + sqlx/sqlite)
frontend/    React 19 + Vite + TypeScript
legacy/      旧版 Next.js remote client 归档
```

## API 摘要

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/healthz` | 健康检查 |
| GET | `/api/v1/domains` | 可用域名列表 |
| POST | `/api/v1/mailboxes` | 创建邮箱 |
| POST | `/api/v1/sessions` | 登录 |
| GET | `/api/v1/me` | 当前邮箱信息 |
| DELETE | `/api/v1/mailboxes/:id` | 删除邮箱 |
| GET | `/api/v1/messages?limit=&offset=` | 消息列表（分页） |
| GET | `/api/v1/messages/:id` | 消息详情 |
| PATCH | `/api/v1/messages/:id` | 更新已读状态 |
| DELETE | `/api/v1/messages/:id` | 删除消息 |
| GET | `/api/v1/events?mailboxId=&token=` | SSE 事件流 |
| POST | `/api/v1/inbound/messages` | HTTP 投递入口 |

完整接口文档见 [frontend/public/llm-api-docs.txt](frontend/public/llm-api-docs.txt)。

## 环境变量

详见 [.env.example](.env.example)，关键项：

- `KOOIXMAIL_DOMAINS` — 允许创建邮箱的域名列表
- `INGRESS_TOKEN` — 投递入口 token（留空则不校验）
- `SMTP_BIND_ADDR` — SMTP 监听地址（留空禁用）
- `SMTP_TLS_MODE` — `disabled | starttls | require-starttls | implicit`

## MX 接入

- 开发：直接投递到 `127.0.0.1:2525`
- 生产：`SMTP_BIND_ADDR=0.0.0.0:25` + `SMTP_TLS_MODE=require-starttls` + 证书 + MX 记录

## 已知限制

- 附件仅返回空数组（待实现持久化）
- Greylisting / rate limit 状态为进程内存，重启丢失
- SSE 事件总线为进程内 broadcast，单实例有效
