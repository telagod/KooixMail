# KooixMail ✉️

> 一个你可以完全掌控的临时邮箱服务。不依赖任何第三方，开箱即用。

KooixMail 让你在自己的机器上运行一套完整的临时邮箱系统 — 创建邮箱、收发邮件、实时推送，全部数据留在本地。

---

## 它能做什么？

🔒 **邮箱管理** — 随时创建临时邮箱，用完即删，密码用 Argon2 加密存储

📬 **收邮件** — 支持 HTTP 投递和标准 SMTP 协议，可以直接接公网 MX 记录

⚡ **实时推送** — 新邮件到达时浏览器即时通知，断线自动重连

🛡️ **反垃圾邮件** — 内置 SPF/DKIM/DMARC 校验、速率限制、灰名单、黑名单查询

🎨 **好看的界面** — 液态玻璃风格 UI，支持深色/浅色主题，自动跟随系统偏好

📱 **响应式** — 桌面、平板、手机都能用

---

## 30 秒启动

你需要：[Rust](https://rustup.rs/) + [Node.js](https://nodejs.org/)

```bash
git clone https://github.com/telagod/KooixMail.git
cd KooixMail
make dev
```

然后打开 http://127.0.0.1:5173 就能用了。

> 也可以分开启动：`cd backend && cargo run` 和 `cd frontend && npm run dev`

想自定义配置？复制 [.env.example](.env.example) 为 `.env` 然后按需修改。

---

## 项目结构

```
KooixMail/
├── backend/      ← Rust 后端 (axum + SQLite)
├── frontend/     ← React 前端 (Vite + TypeScript)
├── legacy/       ← 旧版归档，不参与运行
├── Makefile      ← 一键启动/测试/构建
└── .env.example  ← 环境变量模板
```

---

## API 一览

所有接口都在 `/api/v1` 下：

| 做什么 | 怎么调 |
|--------|--------|
| 创建邮箱 | `POST /mailboxes` |
| 登录 | `POST /sessions` |
| 查看当前邮箱 | `GET /me` |
| 删除邮箱 | `DELETE /mailboxes/:id` |
| 收件列表 | `GET /messages?limit=50&offset=0` |
| 读一封邮件 | `GET /messages/:id` |
| 标记已读/未读 | `PATCH /messages/:id` |
| 删除邮件 | `DELETE /messages/:id` |
| 实时事件流 | `GET /events?mailboxId=...&token=...` |
| 投递邮件 | `POST /inbound/messages` |
| 健康检查 | `GET /healthz` |
| 可用域名 | `GET /domains` |

完整接口文档：[frontend/public/llm-api-docs.txt](frontend/public/llm-api-docs.txt)

---

## 收邮件

KooixMail 有两种方式接收邮件：

**方式一：HTTP 投递**（适合测试和内部集成）
```bash
curl -X POST http://127.0.0.1:3000/api/v1/inbound/messages \
  -H "Content-Type: application/json" \
  -d '{"to":"test@kooixmail.local","fromAddress":"me@example.com","subject":"Hello","text":"Hi!"}'
```

**方式二：SMTP 收信**（适合接公网邮件）
- 开发环境：默认监听 `127.0.0.1:2525`
- 生产环境：改为 `0.0.0.0:25`，开启 TLS，配好 MX 记录

---

## 关键配置

| 变量 | 说明 | 默认值 |
|------|------|--------|
| `KOOIXMAIL_DOMAINS` | 允许的邮箱域名 | `kooixmail.local,quack.local` |
| `INGRESS_TOKEN` | 投递接口密钥，留空不校验 | 空 |
| `SMTP_BIND_ADDR` | SMTP 监听地址，留空禁用 | `127.0.0.1:2525` |
| `SMTP_TLS_MODE` | TLS 模式 | `disabled` |

更多配置见 [.env.example](.env.example)。

---

## 测试

```bash
make test
```

后端 90 个测试覆盖了认证、数据库、全部 API 路由、邮件投递策略和 SMTP 协议。

---

## 已知限制

- 📎 附件功能尚未实现（接口已预留，当前返回空数组）
- 🔄 速率限制和灰名单数据存在内存中，重启后丢失
- 📡 实时推送基于进程内广播，仅支持单实例部署

---

## 许可

MIT
