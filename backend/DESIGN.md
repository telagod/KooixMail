# KooixMail Backend Design

## 设计目标

用最小可行 Rust 服务替代旧的远程代理，形成可运行的临时邮箱内核。

## 方案选择

### 选型

- `axum`
  - 路由、JSON、SSE 简洁，适合小而完整的 API 服务
- `sqlx + sqlite`
  - 单机落地快，适合本地开发与 MVP 持久化
- `argon2`
  - 对 mailbox password 做 hash，避免明文存储

## 关键决策

### 1. 使用 session token，而不是 JWT

- 理由
  - 实现简单，删除 mailbox 时可直接失效
  - 不需要额外签名密钥与 token 轮换

### 2. inbound 统一走共享落库链路，HTTP 与 SMTP 共用

- 理由
  - 避免 HTTP bridge 与 SMTP DATA 处理各自维护一套 message 落库逻辑
  - 后续补附件、spam filter、DKIM 时只需要沿同一条 inbound pipeline 扩展

### 3. 先内置最小 SMTP ingress，再逐步补公网邮件治理

- 理由
  - 用户已经要求从 remote 关系转入公网收信链路，必须先把 SMTP/MX 入口打通
  - 先用 `smtpd` + `mail-parser` 接住 RCPT / DATA，再把反垃圾与认证校验作为下一阶段

### 5. STARTTLS、SPF/DKIM/DMARC 与 anti-abuse 统一收敛到 inbound pipeline

- 理由
  - SMTP 只是入口，真正的门禁必须落在共享 ingest 链，HTTP bridge 与 SMTP 才不会分叉
  - `STARTTLS` 与 25 端口策略属于 listener 层约束；SPF/DKIM/DMARC、限速、local-domain spoof protection 属于消息接纳层约束
  - 这样后续接 greylisting、RBL、内容过滤时，仍然只需要扩展一条链路

### 4. SSE 采用 query token

- 理由
  - 浏览器原生 `EventSource` 不支持自定义 Authorization header
  - 当前服务是本地单机场景，易于前端接入

## 安全边界

- 密码仅保存 `argon2` hash
- 删除 mailbox 时依赖 SQLite foreign key 级联清理 session/message
- inbound endpoint 可通过 `INGRESS_TOKEN` 收紧
- SMTP `RCPT TO` 仅接受已存在且未过期的本地域 mailbox
- STARTTLS 通过 `SMTP_TLS_MODE=disabled|starttls|require-starttls|implicit` 控制
- 当监听 `:25` 时，`SMTP_TLS_MODE=disabled` 将被硬拒绝启动
- SMTP ingress 通过 `mail-auth` 将 SPF / DKIM / DMARC verdict 注入共享 inbound gate
- anti-abuse 包括 message size limit、per-IP rate limit、本地域 spoof protection、greylisting、RBL DNS 查询
- 当前 query token 仅用于本地 SSE，公网部署前应改为更严格的事件鉴权桥

## 已知限制

- 附件仅返回空数组，未做二进制存储
- 事件总线为进程内 broadcast，单实例有效
- SPF/DKIM/DMARC 当前依赖运行节点的 DNS 解析与本机 resolver 健康度
- greylisting 与 rate limit 状态均为进程内存，重启丢失，仅适用于单实例
- RBL 查询依赖运行节点 DNS resolver 可达 DNSBL 服务
- 尚未实现内容级 spam filter（贝叶斯/关键词）

## 变更历史

### 2026-03-20 - Rust 本地邮箱后端落地

**变更内容**: 新增 mailbox/session/message/SSE/inbound 最小闭环

**变更理由**: 斩断 KooixMail API、Mail.tm、Mercure 等远程依赖

**影响范围**: 前端 API 契约、仓库启动方式、数据存储模型

**决策依据**: 先用 SQLite 与 HTTP bridge 快速形成自管邮箱内核，再逐步补 SMTP ingress

### 2026-03-20 - 后端拆分为四模块

**变更内容**: 将单文件实现拆为 `routes / auth / db / models`

**变更理由**: 降低单文件复杂度，明确职责边界，便于后续接入 SMTP ingress

**影响范围**: 代码组织结构、启动编排与内部依赖关系

**决策依据**: 保持 API 不变，只做结构重构，让行为面与分层面解耦

### 2026-03-20 - db 子模块拆分并接入 SMTP ingress

**变更内容**: 将 `db.rs` 继续拆为 `runtime / mailboxes / sessions / messages`，新增共享 `inbound` 流程与 `smtp.rs`

**变更理由**: 为公网收信链路预留明确边界，避免 SMTP 与 HTTP 重复写入逻辑

**影响范围**: 运行时配置、消息落库链路、MX 接入方式、模块文档

**决策依据**: 先形成最小可运行的 SMTP/MX ingress，再逐步补齐反垃圾与邮件身份校验

### 2026-03-20 - STARTTLS 与 inbound auth gate 接线

**变更内容**: 新增 `SMTP_TLS_MODE` / 证书路径配置，接入 `STARTTLS`，并将 SPF / DKIM / DMARC、rate limit、local-domain spoof protection 收敛到共享 inbound pipeline；补充真实 socket SMTP integration test

**变更理由**: 公网 25 端口投递不能只停留在明文开发 listener，且身份校验与 anti-abuse 不能继续悬空

**影响范围**: SMTP 运行时配置、共享 ingest policy、测试覆盖范围、部署文档

**决策依据**: listener 层负责 TLS，message pipeline 层负责门禁，两层分离但共用一套接纳决策

### 2026-03-21 - Greylisting、RBL、:25 硬拒绝与真实 TLS 握手测试

**变更内容**: 新增 greylisting（(IP, sender, recipient) 三元组延迟投递）、RBL DNS 查询（DNSBL 命中即拒绝）、`:25 + Disabled TLS` 启动硬拒绝；补充真实 STARTTLS TLS 握手后完整加密 SMTP 对话测试

**变更理由**: 上一轮仅有 warn 级别的 :25 明文告警，greylisting 与 RBL 作为公网收信的基础反垃圾手段不可缺失；STARTTLS 测试此前仅验证广告，未完成真实 TLS 升级后的投递验证

**影响范围**: inbound pipeline 策略链、SMTP 启动校验、AppConfig/AppState 扩展、测试覆盖、部署文档

**决策依据**: greylisting 在 inbound gate 层执行（与 rate limit 同层），RBL 在 rate limit 之前执行以尽早拒绝已知恶意 IP；:25 硬拒绝确保生产不会意外明文暴露

### 2026-03-21 - 全面打磨：并发安全、高负载、UX、正确性

**变更内容**:
- SMTP 多收件人部分失败不再 early return，逐个投递，部分成功即返回 OK
- SQLite 启用 WAL journal mode + busy_timeout，连接池扩容至 16 + acquire_timeout
- 新增 sessions.mailbox_id 索引与 mailboxes.expires_at 部分索引
- cleanup worker 扩展：定期清理 ingress_limits / greylist / events 三个内存 HashMap
- messages 列表接口加分页（limit/offset），默认 50 条，上限 200
- update_message 减少一次 DB 往返（patch in-memory row）
- ingest_inbound_message 移除多余 re-fetch（insert 后直接构造 MessageRow）
- ingress token 比较改为常量时间
- UNIQUE 约束检测改用 sqlx 结构化错误码
- password 校验不再 trim（空格密码不再绕过）
- calculate_expiry 负值改为返回 BadRequest 而非静默回退
- event_sender 改为读锁优先，减少写锁争用
- delete_mailbox 时主动清理 events HashMap
- ContactResponse.to.name 改为实际收件人地址
- 畸形 RFC822 消息加 warn 日志

**变更理由**: 审计发现 30+ 处薄弱点，涵盖并发安全（全局写锁争用、内存泄漏）、高负载（连接池不足、无分页、无 WAL）、正确性（多收件人中断、timing oracle、密码绕过）、UX（硬编码 to name、负值静默回退）

**影响范围**: db/runtime、inbound pipeline、SMTP handler、routes、auth、models

**决策依据**: 按 CRITICAL > HIGH > MEDIUM 优先级分批修复，确保每批修复后测试全绿再进入下一批
