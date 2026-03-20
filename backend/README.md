# KooixMail Backend

## 模块定位

Rust 临时邮箱后端，负责 mailbox 生命周期、session token、消息存储、SSE 事件、HTTP inbound 与 SMTP ingress。

## 为什么存在

旧项目只是 remote API 代理，没有任何自管数据面。此模块把核心邮箱能力收回本地。

## 核心职责

- 创建、登录、删除 mailbox
- 持久化 message 到 SQLite
- 对前端暴露本地 REST API 与 SSE
- 提供受控 inbound endpoint，供内部 bridge 或测试写入
- 提供基础 SMTP ingress，可直接承接公网 MX 投递
- 在 HTTP / SMTP 共用 inbound gate 上执行 size limit、rate limit 与 sender auth policy

## 模块结构

- `src/routes.rs`
  - router、handlers、SSE 事件分发、响应映射
- `src/auth.rs`
  - token 提取、password hash/verify、地址与过期校验
- `src/db/`
  - `runtime.rs`：SQLite 连接、迁移、过期清理 worker
  - `mailboxes.rs`：mailbox 查询与删除
  - `sessions.rs`：session token 持久化与查找
  - `messages.rs`：message 存取与状态更新
- `src/models.rs`
  - 共享类型、请求/响应 DTO、row model、统一错误
- `src/inbound.rs`
  - HTTP / SMTP 共用的 inbound 落库链路
- `src/smtp.rs`
  - SMTP listener、RCPT 校验、DATA 解析与写入
- `src/lib.rs`
  - 运行时编排与模块接线
- `src/main.rs`
  - 启动入口

## 不负责

- 文件附件持久化
- 内容级 spam filter（贝叶斯/关键词）

## 依赖关系

- 依赖 `axum`、`tokio`、`sqlx/sqlite`、`argon2`
- 被 `frontend/` 直接调用

## 快速启动

```bash
cargo run
```

默认地址：`http://127.0.0.1:3000`

默认 SMTP ingress：`127.0.0.1:2525`

## SMTP / MX

- 开发环境
  - 默认监听 `SMTP_BIND_ADDR=127.0.0.1:2525`
  - 可用任意 SMTP client 直接投递到本地 listener
- 公网环境
  - 将 `SMTP_BIND_ADDR` 设为 `0.0.0.0:25`
  - 将 `SMTP_TLS_MODE` 设为 `require-starttls`
  - 提供 `SMTP_TLS_CERT_PATH` 与 `SMTP_TLS_KEY_PATH`
  - 将 `SMTP_HOSTNAME` 设为公网可解析的 MX 主机名，例如 `mx.example.com`
  - 将目标域的 `MX` 记录指向该主机，并保证其 `A/AAAA` 解析到本机
  - 当前已接入基础 size limit、rate limit、SPF/DKIM/DMARC gate 与本地域 spoof protection

## Ingress 策略开关

- `INGRESS_MAX_MESSAGE_BYTES`
  - SMTP / HTTP inbound 共用的最大消息大小
- `INGRESS_RATE_LIMIT_PER_MINUTE`
  - 以 `remote IP` 优先、sender 次之的每分钟限速
- `INGRESS_REQUIRE_SPF`
  - 要求 SMTP ingress 的 SPF 判定为 `Pass`
- `INGRESS_REQUIRE_DKIM`
  - 要求 SMTP ingress 的 DKIM 判定为 `Pass`
- `INGRESS_REQUIRE_DMARC`
  - 要求 SMTP ingress 的 DMARC 对齐结果为 `Pass`
- `INGRESS_PROTECT_LOCAL_DOMAINS`
  - 当 header-from 或 sender 伪装成本地域时，至少要求 SPF / DKIM / DMARC 一项通过
- `INGRESS_GREYLIST_ENABLED`
  - 启用灰名单；首次见到的 (IP, sender, recipient) 三元组将被临时拒绝
- `INGRESS_GREYLIST_DELAY_SECS`
  - 灰名单延迟秒数，默认 `60`
- `INGRESS_RBL_ZONES`
  - 逗号分隔的 DNSBL 域名列表，如 `zen.spamhaus.org,bl.spamcop.net`；留空则不查询

## 测试

```bash
cargo test    # 90 个测试
```

覆盖范围：
- `auth.rs` — 地址规范化、密码校验、过期检查、token 提取、hash 往返（32 个）
- `db/` — 全部 CRUD + 级联删除 + 分页排序（16 个）
- `routes.rs` — 12 条 API 路由全覆盖，含认证/越权/冲突/ingress token（21 个）
- `inbound.rs` — 投递 + 速率限制 + 本地域防伪 + Greylist + trusted 跳过 + 超限拒绝（8 个）
- `smtp.rs` — TCP SMTP 对话 + STARTTLS 握手 + TLS 配置校验（7 个）
- `lib.rs` — 配置解析（5 个）
