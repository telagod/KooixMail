# KooixMail ✉️

> A temporary mailbox service you fully control. No third-party dependencies. Works out of the box.

KooixMail lets you run a complete temporary email system on your own machine — create mailboxes, receive emails, get real-time notifications. All data stays local.

---

## What can it do?

🔒 **Mailbox Management** — Create disposable mailboxes on the fly, delete when done. Passwords stored with Argon2.

📬 **Receive Emails** — Via HTTP endpoint or standard SMTP protocol. Can accept mail from the public internet via MX records.

⚡ **Real-time Push** — Browser gets notified instantly when new mail arrives. Auto-reconnects if connection drops.

🛡️ **Anti-Spam** — Built-in SPF/DKIM/DMARC verification, rate limiting, greylisting, and DNSBL lookups.

🎨 **Beautiful UI** — Liquid glass aesthetic with dark/light theme toggle that follows your system preference.

📱 **Responsive** — Works on desktop, tablet, and mobile.

---

## Get Running in 30 Seconds

You need: [Rust](https://rustup.rs/) + [Node.js](https://nodejs.org/)

```bash
git clone https://github.com/telagod/KooixMail.git
cd KooixMail
make dev
```

Then open http://127.0.0.1:5173 and you're good to go.

> You can also start them separately: `cd backend && cargo run` and `cd frontend && npm run dev`

Want to customize? Copy [.env.example](.env.example) to `.env` and tweak as needed.

---

## Project Structure

```
KooixMail/
├── backend/      ← Rust backend (axum + SQLite)
├── frontend/     ← React frontend (Vite + TypeScript)
├── legacy/       ← Archived old version, not used
├── Makefile      ← One-command dev/test/build
└── .env.example  ← Environment variable template
```

---

## API at a Glance

All endpoints live under `/api/v1`:

| What | How |
|------|-----|
| Create mailbox | `POST /mailboxes` |
| Login | `POST /sessions` |
| Current mailbox | `GET /me` |
| Delete mailbox | `DELETE /mailboxes/:id` |
| List messages | `GET /messages?limit=50&offset=0` |
| Read a message | `GET /messages/:id` |
| Mark read/unread | `PATCH /messages/:id` |
| Delete message | `DELETE /messages/:id` |
| Live event stream | `GET /events?mailboxId=...&token=...` |
| Deliver a message | `POST /inbound/messages` |
| Health check | `GET /healthz` |
| Available domains | `GET /domains` |

Full API docs: [frontend/public/llm-api-docs.txt](frontend/public/llm-api-docs.txt)

---

## Receiving Email

KooixMail accepts email in two ways:

**Option 1: HTTP Delivery** (great for testing and internal integrations)
```bash
curl -X POST http://127.0.0.1:3000/api/v1/inbound/messages \
  -H "Content-Type: application/json" \
  -d '{"to":"test@kooixmail.local","fromAddress":"me@example.com","subject":"Hello","text":"Hi!"}'
```

**Option 2: SMTP Ingress** (for receiving real email from the internet)
- Development: listens on `127.0.0.1:2525` by default
- Production: switch to `0.0.0.0:25`, enable TLS, set up MX records

---

## Key Configuration

| Variable | What it does | Default |
|----------|-------------|---------|
| `KOOIXMAIL_DOMAINS` | Allowed mailbox domains | `kooixmail.local,quack.local` |
| `INGRESS_TOKEN` | Delivery endpoint secret, empty = no auth | empty |
| `SMTP_BIND_ADDR` | SMTP listen address, empty = disabled | `127.0.0.1:2525` |
| `SMTP_TLS_MODE` | TLS mode | `disabled` |

See [.env.example](.env.example) for all options.

---

## Testing

```bash
make test
```

90 backend tests covering authentication, database operations, all API routes, email delivery policies, and SMTP protocol.

---

## Known Limitations

- 📎 Attachments not yet implemented (API is ready, currently returns empty arrays)
- 🔄 Rate limit and greylist state lives in memory, lost on restart
- 📡 Real-time push uses in-process broadcast, single-instance only

---

## License

MIT
