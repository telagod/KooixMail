# KooixMail

KooixMail has been rewritten from a `Next.js + remote API proxy` into a `Rust backend + React frontend` local temporary mail service.

## Architecture

- `backend/`
  - `axum + tokio + sqlx(sqlite)`
  - owns mailboxes, session tokens, messages, and SSE events
  - controlled inbound endpoint: `POST /api/v1/inbound/messages`
- `frontend/`
  - `React 19 + Vite + TypeScript`
  - single-page inbox wired directly to the local Rust API
  - local multi-mailbox session persistence, no provider switching or API keys
- `legacy/next-remote/`
  - archived Next.js remote client for migration reference only

## Run

```bash
npm run dev:backend
npm run dev:frontend
```

Frontend defaults to `http://127.0.0.1:5173`.
Backend defaults to `http://127.0.0.1:3000`.

## Environment

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

- `SMTP_TLS_MODE`
  - `disabled | starttls | require-starttls | implicit`
- `SMTP_TLS_CERT_PATH` / `SMTP_TLS_KEY_PATH`
  - required when STARTTLS or implicit TLS is enabled
- `INGRESS_MAX_MESSAGE_BYTES`
  - shared inbound size cap for HTTP and SMTP
- `INGRESS_RATE_LIMIT_PER_MINUTE`
  - basic per-remote-IP throttle, `0` disables it
- `INGRESS_REQUIRE_SPF` / `INGRESS_REQUIRE_DKIM` / `INGRESS_REQUIRE_DMARC`
  - promote authentication verdicts into the shared inbound gate
- `INGRESS_PROTECT_LOCAL_DOMAINS`
  - reject spoofed local sender domains unless SPF, DKIM, or DMARC passes
- `INGRESS_GREYLIST_ENABLED`
  - enable greylisting; first-seen (IP, sender, recipient) tuples are temporarily rejected
- `INGRESS_GREYLIST_DELAY_SECS`
  - greylist delay in seconds, default `60`
- `INGRESS_RBL_ZONES`
  - comma-separated DNSBL zone list, e.g. `zen.spamhaus.org,bl.spamcop.net`; empty disables RBL

## API

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

The AI-oriented API reference now lives at [frontend/public/llm-api-docs.txt](frontend/public/llm-api-docs.txt).

## Current boundary

The rewrite already removes all remote-provider dependencies. What remains is the public delivery edge:

- done: mailbox lifecycle, login, message storage, SSE refresh, controlled inbound delivery, SMTP ingress, STARTTLS wiring, anti-abuse (rate limit, size limit, local-domain spoof protection, greylisting, RBL), SPF/DKIM/DMARC gate, port 25 + no-TLS hard reject
- pending: production DNS/certificate rollout, attachments, and content-level spam filter

Production port 25 baseline:

- set `SMTP_BIND_ADDR=0.0.0.0:25`
- set `SMTP_TLS_MODE=require-starttls`
- provide `SMTP_TLS_CERT_PATH` and `SMTP_TLS_KEY_PATH`
- point your domain `MX` record at `SMTP_HOSTNAME`

Test coverage now includes real socket-level SMTP dialogue and `STARTTLS` advertisement/response.
