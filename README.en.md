# KooixMail

Self-hosted temporary mailbox service — Rust backend + React frontend, zero third-party dependencies.

## Features

- Mailbox create/login/delete with Argon2 password hashing
- Messages: receive/paginate/read-unread/delete
- SSE real-time push (auto-reconnect with exponential backoff)
- HTTP inbound endpoint + SMTP ingress (STARTTLS / implicit TLS)
- SPF / DKIM / DMARC verification, local domain spoof protection
- Per-IP rate limiting, message size limit, greylisting, RBL/DNSBL
- Liquid glass UI with dark/light theme toggle (system preference + persistence)
- Responsive layout (1320px / 960px / 480px breakpoints)
- Accessibility: aria-live notifications, aria-label cards, keyboard navigation

## Quick Start

```bash
# One-command start (requires Rust + Node.js)
make dev

# Or start separately
cd backend && cargo run        # http://127.0.0.1:3000
cd frontend && npm run dev     # http://127.0.0.1:5173
```

See [.env.example](.env.example) for environment variables.

## Testing

```bash
make test

# Or separately
cd backend && cargo test       # 90 tests
cd frontend && npx tsc --noEmit
```

Coverage:
- `auth.rs` — address normalization, password validation, expiry, token extraction, hash roundtrip (32)
- `db/` — full CRUD + cascade delete + pagination ordering (16)
- `routes.rs` — all 12 API routes with auth/forbidden/conflict/ingress token (21)
- `inbound.rs` — delivery + rate limit + local domain spoof + greylist + trusted bypass + oversize reject (8)
- `smtp.rs` — TCP SMTP dialogue + STARTTLS handshake + TLS config validation (7)
- `lib.rs` — config parsing (5)

## Architecture

```
backend/     Rust (axum + tokio + sqlx/sqlite)
frontend/    React 19 + Vite + TypeScript
legacy/      Archived Next.js remote client
```

## API

| Method | Path | Description |
|--------|------|-------------|
| GET | `/healthz` | Health check |
| GET | `/api/v1/domains` | Available domains |
| POST | `/api/v1/mailboxes` | Create mailbox |
| POST | `/api/v1/sessions` | Login |
| GET | `/api/v1/me` | Current mailbox info |
| DELETE | `/api/v1/mailboxes/:id` | Delete mailbox |
| GET | `/api/v1/messages?limit=&offset=` | List messages (paginated) |
| GET | `/api/v1/messages/:id` | Message detail |
| PATCH | `/api/v1/messages/:id` | Update read status |
| DELETE | `/api/v1/messages/:id` | Delete message |
| GET | `/api/v1/events?mailboxId=&token=` | SSE event stream |
| POST | `/api/v1/inbound/messages` | HTTP delivery endpoint |

Full API reference at [frontend/public/llm-api-docs.txt](frontend/public/llm-api-docs.txt).

## Environment

See [.env.example](.env.example). Key variables:

- `KOOIXMAIL_DOMAINS` — allowed mailbox domains (comma-separated)
- `INGRESS_TOKEN` — delivery endpoint token (empty = no auth)
- `SMTP_BIND_ADDR` — SMTP listen address (empty = disabled)
- `SMTP_TLS_MODE` — `disabled | starttls | require-starttls | implicit`

## MX Setup

- Dev: deliver to `127.0.0.1:2525`
- Production: `SMTP_BIND_ADDR=0.0.0.0:25` + `SMTP_TLS_MODE=require-starttls` + certs + MX record

## Known Limitations

- Attachments return empty array (persistence not yet implemented)
- Greylisting / rate limit state is in-process memory, lost on restart
- SSE event bus is in-process broadcast, single-instance only
