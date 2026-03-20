# KooixMail

KooixMail 现已从 `Next.js + remote API 代理` 重写为 `Rust backend + React frontend` 的本地临时邮件服务骨架。

## 当前架构

- `backend/`
  - `axum + tokio + sqlx(sqlite)`
  - 自管 mailbox、session token、message store、SSE 事件流
  - 受控 inbound endpoint：`POST /api/v1/inbound/messages`
  - 内置 SMTP ingress，可直接承接 MX 投递
- `frontend/`
  - `React 19 + Vite + TypeScript`
  - 单页 inbox，直接连接本地 Rust API
  - 本地持久化多邮箱会话，不再暴露 provider / API Key / Mercure
- `legacy/next-remote/`
  - 旧版 Next.js remote client 归档，仅供对照迁移，不再参与当前运行链路

## 快速启动

### 1. 启动 Rust backend

```bash
npm run dev:backend
```

默认监听 `http://127.0.0.1:3000`，使用 `sqlite://kooixmail.db`。

### 2. 启动 React frontend

```bash
npm run dev:frontend
```

默认访问 `http://127.0.0.1:5173`，前端会请求 `http://127.0.0.1:3000/api/v1`。

## 关键环境变量

```bash
PORT=3000
DATABASE_URL=sqlite://kooixmail.db
KOOIXMAIL_DOMAINS=kooixmail.local,quack.local
INGRESS_TOKEN=
SMTP_BIND_ADDR=127.0.0.1:2525
SMTP_HOSTNAME=mx.kooixmail.local
SMTP_TLS_MODE=disabled
SMTP_TLS_CERT_PATH=
SMTP_TLS_KEY_PATH=
INGRESS_MAX_MESSAGE_BYTES=262144
INGRESS_RATE_LIMIT_PER_MINUTE=30
INGRESS_REQUIRE_SPF=false
INGRESS_REQUIRE_DKIM=false
INGRESS_REQUIRE_DMARC=false
INGRESS_PROTECT_LOCAL_DOMAINS=true
INGRESS_GREYLIST_ENABLED=false
INGRESS_GREYLIST_DELAY_SECS=60
INGRESS_RBL_ZONES=
```

- `KOOIXMAIL_DOMAINS`
  - 允许创建邮箱的域名列表，逗号分隔
- `INGRESS_TOKEN`
  - 若设置，调用 inbound endpoint 时必须带 `X-Ingress-Token`
- `SMTP_BIND_ADDR`
  - SMTP ingress 监听地址；设为空可禁用 SMTP listener
- `SMTP_HOSTNAME`
  - SMTP greeting / MX 主机名；生产应设置为公网可解析主机
- `SMTP_TLS_MODE`
  - `disabled | starttls | require-starttls | implicit`
- `SMTP_TLS_CERT_PATH` / `SMTP_TLS_KEY_PATH`
  - 启用 STARTTLS 或 implicit TLS 时必须提供 PEM 证书与私钥
- `INGRESS_MAX_MESSAGE_BYTES`
  - SMTP / HTTP inbound 共用的最大消息体大小
- `INGRESS_RATE_LIMIT_PER_MINUTE`
  - 以 `remote IP` 为主键的基础限速；设为 `0` 可关闭
- `INGRESS_REQUIRE_SPF` / `INGRESS_REQUIRE_DKIM` / `INGRESS_REQUIRE_DMARC`
  - 将邮件身份校验结果接入共用 inbound gate
- `INGRESS_PROTECT_LOCAL_DOMAINS`
  - 当 `From` 冒充本地域时，要求 SPF / DKIM / DMARC 至少一项通过
- `INGRESS_GREYLIST_ENABLED`
  - 启用灰名单；首次见到的 (IP, sender, recipient) 三元组将被临时拒绝
- `INGRESS_GREYLIST_DELAY_SECS`
  - 灰名单延迟秒数，默认 `60`
- `INGRESS_RBL_ZONES`
  - 逗号分隔的 DNSBL 域名列表，如 `zen.spamhaus.org,bl.spamcop.net`；留空则不查询

## API 摘要

- `GET /healthz`
- `GET /api/v1/domains`
- `POST /api/v1/mailboxes`
- `POST /api/v1/sessions`
- `GET /api/v1/me`
- `DELETE /api/v1/mailboxes/:id`
- `GET /api/v1/messages`
- `GET /api/v1/messages/:id`
- `PATCH /api/v1/messages/:id`
- `DELETE /api/v1/messages/:id`
- `GET /api/v1/events?mailboxId=...&token=...`
- `POST /api/v1/inbound/messages`

完整 AI 友好接口文档见 [frontend/public/llm-api-docs.txt](/home/telagod/project/k2i/KooixMail/frontend/public/llm-api-docs.txt)。

## 当前边界

这次重写已经斩断所有 remote provider 依赖，并已开始接入真实公网收信链路：

- 已具备：邮箱创建、登录、消息存储、SSE 推送、HTTP inbound、SMTP ingress、STARTTLS 配置、基础 rate limit、SPF/DKIM/DMARC gate 接线
- 仍待补：真实 DNS/证书下的生产联调、附件持久化、更完整的 anti-abuse（greylisting/RBL/content filter）

MX 接入方式：

- 开发：直接投递到 `127.0.0.1:2525`
- 生产：
  - 把 `SMTP_BIND_ADDR` 设为 `0.0.0.0:25`
  - 把 `SMTP_TLS_MODE` 至少设为 `require-starttls`
  - 配置 `SMTP_TLS_CERT_PATH` 与 `SMTP_TLS_KEY_PATH`
  - 把域名 `MX` 指向 `SMTP_HOSTNAME` 对应的 `A/AAAA`

测试现状：

- `cargo test --manifest-path backend/Cargo.toml`
  - 已覆盖真实 TCP socket SMTP 对话落库
  - 已覆盖 `EHLO -> STARTTLS` 广告与响应

因此它现在是一个“本地自管临时邮箱内核 + UI + 最小 SMTP 收信入口”，不再是依赖第三方的 remote 壳。
