import { useCallback, useEffect, useMemo, useState, type FormEvent } from "react"

import {
  API_BASE_URL,
  createMailbox,
  createSession,
  deleteMailbox,
  deleteMessage,
  deliverMessage,
  getCurrentMailbox,
  getMessage,
  listDomains,
  listMessages,
  markMessageSeen,
  subscribeMailboxEvents,
} from "./lib/api"
import { loadActiveMailboxId, loadSessions, saveActiveMailboxId, saveSessions } from "./lib/storage"
import { applyTheme, getInitialTheme, toggleTheme, type Theme } from "./lib/theme"
import type { AuthSession, Domain, MessageDetail, MessageSummary } from "./types"

const expiryOptions = [
  { label: "1 小时", value: 60 * 60 },
  { label: "24 小时", value: 24 * 60 * 60 },
  { label: "7 天", value: 7 * 24 * 60 * 60 },
  { label: "永不过期", value: 0 },
]

type NoticeTone = "neutral" | "success" | "danger"

function createRandomToken(length: number) {
  const chars = "abcdefghijkmnpqrstuvwxyz23456789"
  const bytes = crypto.getRandomValues(new Uint32Array(length))
  return Array.from(bytes, (value) => chars[value % chars.length]).join("")
}

function formatDate(value: string | null) {
  if (!value) return "永不过期"
  return new Intl.DateTimeFormat("zh-CN", {
    dateStyle: "medium",
    timeStyle: "short",
  }).format(new Date(value))
}

function formatRelative(value: string) {
  const date = new Date(value)
  const delta = Date.now() - date.getTime()
  const minutes = Math.round(delta / 60000)
  if (minutes < 1) return "刚刚"
  if (minutes < 60) return `${minutes} 分钟前`
  const hours = Math.round(minutes / 60)
  if (hours < 24) return `${hours} 小时前`
  const days = Math.round(hours / 24)
  return `${days} 天前`
}

function upsertSession(sessions: AuthSession[], nextSession: AuthSession) {
  const withoutCurrent = sessions.filter((session) => session.mailbox.id !== nextSession.mailbox.id)
  return [nextSession, ...withoutCurrent]
}

function App() {
  const [theme, setTheme] = useState<Theme>(() => {
    const initial = getInitialTheme()
    applyTheme(initial)
    return initial
  })

  function handleToggleTheme() {
    const next = toggleTheme()
    setTheme(next)
  }

  const [domains, setDomains] = useState<Domain[]>([])
  const [sessions, setSessions] = useState<AuthSession[]>(() => loadSessions())
  const [activeMailboxId, setActiveMailboxId] = useState<string | null>(() => loadActiveMailboxId())
  const [messages, setMessages] = useState<MessageSummary[]>([])
  const [selectedMessageId, setSelectedMessageId] = useState<string | null>(null)
  const [selectedMessage, setSelectedMessage] = useState<MessageDetail | null>(null)
  const [busy, setBusy] = useState(false)
  const [notice, setNotice] = useState<{ tone: NoticeTone; text: string }>({
    tone: "neutral",
    text: "本地 Rust backend 已接管邮箱、消息、SSE 与临时数据存储。",
  })
  const [createForm, setCreateForm] = useState({
    username: createRandomToken(8),
    domain: "",
    password: createRandomToken(12),
    expiresIn: expiryOptions[1].value,
  })
  const [loginForm, setLoginForm] = useState({
    address: "",
    password: "",
  })
  const [deliveryForm, setDeliveryForm] = useState({
    fromAddress: "sender@outer.net",
    fromName: "Dark Relay",
    subject: "KooixMail 本地投递校验",
    text: "这是一封通过 Rust inbound endpoint 注入的测试邮件。",
  })
  const [createdCredentials, setCreatedCredentials] = useState<{ address: string; password: string } | null>(null)
  const [hasMore, setHasMore] = useState(false)
  const PAGE_SIZE = 50

  const activeSession = useMemo(
    () => sessions.find((session) => session.mailbox.id === activeMailboxId) ?? null,
    [sessions, activeMailboxId],
  )

  useEffect(() => {
    saveSessions(sessions)
  }, [sessions])

  useEffect(() => {
    saveActiveMailboxId(activeMailboxId)
  }, [activeMailboxId])

  useEffect(() => {
    if (!activeMailboxId && sessions.length > 0) {
      setActiveMailboxId(sessions[0].mailbox.id)
    }
  }, [sessions, activeMailboxId])

  // P1: Session 校验 — 启动时用 /me 验证 token 有效性，清除失效 session
  useEffect(() => {
    if (!activeSession) return
    let cancelled = false

    getCurrentMailbox(activeSession.token).catch(() => {
      if (cancelled) return
      // token 失效，移除该 session
      const nextSessions = sessions.filter((s) => s.mailbox.id !== activeSession.mailbox.id)
      setSessions(nextSessions)
      setActiveMailboxId(nextSessions[0]?.mailbox.id ?? null)
      setNotice({ tone: "danger", text: `邮箱 ${activeSession.mailbox.address} 的会话已失效，已自动移除。` })
    })

    return () => {
      cancelled = true
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeSession?.mailbox.id])

  useEffect(() => {
    let cancelled = false

    listDomains()
      .then((nextDomains) => {
        if (cancelled) return
        setDomains(nextDomains)
        setCreateForm((current) => ({
          ...current,
          domain: current.domain || nextDomains[0]?.domain || "",
        }))
      })
      .catch((error: Error) => {
        if (!cancelled) {
          setNotice({ tone: "danger", text: error.message })
        }
      })

    return () => {
      cancelled = true
    }
  }, [])

  const refreshMessages = useCallback(async (preserveSelection = true) => {
    if (!activeSession) {
      setMessages([])
      setSelectedMessageId(null)
      setSelectedMessage(null)
      setHasMore(false)
      return
    }

    try {
      const nextMessages = await listMessages(activeSession.token, PAGE_SIZE, 0)
      setMessages(nextMessages)
      setHasMore(nextMessages.length >= PAGE_SIZE)

      if (preserveSelection && selectedMessageId && nextMessages.some((item) => item.id === selectedMessageId)) {
        return
      }

      const nextSelected = nextMessages[0]?.id ?? null
      setSelectedMessageId(nextSelected)
      if (!nextSelected) {
        setSelectedMessage(null)
      }
    } catch (error) {
      setNotice({ tone: "danger", text: (error as Error).message })
    }
  }, [activeSession, selectedMessageId])

  async function handleLoadMore() {
    if (!activeSession) return
    try {
      const nextPage = await listMessages(activeSession.token, PAGE_SIZE, messages.length)
      setMessages((current) => [...current, ...nextPage])
      setHasMore(nextPage.length >= PAGE_SIZE)
    } catch (error) {
      setNotice({ tone: "danger", text: (error as Error).message })
    }
  }

  useEffect(() => {
    void refreshMessages(false)
  }, [activeSession?.mailbox.id, refreshMessages])

  useEffect(() => {
    if (!activeSession) return

    let closed = false
    let source: ReturnType<typeof subscribeMailboxEvents> | null = null
    let retryTimeout: ReturnType<typeof setTimeout> | null = null
    let retryCount = 0

    function connect() {
      if (closed) return
      source = subscribeMailboxEvents(activeSession.mailbox.id, activeSession.token, () => {
        void refreshMessages(true)
      })

      source.onopen = () => {
        retryCount = 0
      }

      source.onerror = () => {
        source?.close()
        source = null
        if (closed) return
        retryCount += 1
        const delay = Math.min(1000 * 2 ** retryCount, 30000)
        setNotice({ tone: "danger", text: `实时事件流断开，${Math.round(delay / 1000)}s 后重连...` })
        retryTimeout = setTimeout(connect, delay)
      }
    }

    connect()

    return () => {
      closed = true
      source?.close()
      if (retryTimeout) clearTimeout(retryTimeout)
    }
  }, [activeSession, activeSession?.mailbox.id, activeSession?.token, refreshMessages])

  useEffect(() => {
    if (!activeSession || !selectedMessageId) {
      setSelectedMessage(null)
      return
    }

    let cancelled = false

    getMessage(activeSession.token, selectedMessageId)
      .then(async (message) => {
        if (cancelled) return
        setSelectedMessage(message)
        if (!message.seen) {
          const updated = await markMessageSeen(activeSession.token, message.id, true)
          if (cancelled) return
          setSelectedMessage(updated)
          setMessages((current) =>
            current.map((item) => (item.id === updated.id ? { ...item, seen: true } : item)),
          )
        }
      })
      .catch((error: Error) => {
        if (!cancelled) {
          setNotice({ tone: "danger", text: error.message })
        }
      })

    return () => {
      cancelled = true
    }
  }, [activeSession, activeSession?.token, selectedMessageId])

  async function handleCreateMailbox(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (!createForm.domain) return

    setBusy(true)
    const address = `${createForm.username}@${createForm.domain}`

    try {
      const session = await createMailbox({
        address,
        password: createForm.password,
        expiresIn: createForm.expiresIn,
      })

      setSessions((current) => upsertSession(current, session))
      setActiveMailboxId(session.mailbox.id)
      setCreatedCredentials({ address, password: createForm.password })
      setNotice({ tone: "success", text: `邮箱 ${address} 已由本地服务创建。` })
      setCreateForm((current) => ({
        ...current,
        username: createRandomToken(8),
        password: createRandomToken(12),
      }))
    } catch (error) {
      setNotice({ tone: "danger", text: (error as Error).message })
    } finally {
      setBusy(false)
    }
  }

  async function handleLogin(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    setBusy(true)

    try {
      const session = await createSession(loginForm)
      setSessions((current) => upsertSession(current, session))
      setActiveMailboxId(session.mailbox.id)
      setLoginForm({ address: "", password: "" })
      setNotice({ tone: "success", text: `已接管邮箱 ${session.mailbox.address}。` })
    } catch (error) {
      setNotice({ tone: "danger", text: (error as Error).message })
    } finally {
      setBusy(false)
    }
  }

  async function handleDeleteMailbox() {
    if (!activeSession) return
    const confirmed = window.confirm(`确定要删除邮箱 ${activeSession.mailbox.address} 吗？此操作不可撤销。`)
    if (!confirmed) return
    setBusy(true)

    try {
      await deleteMailbox(activeSession.token, activeSession.mailbox.id)
      const nextSessions = sessions.filter((session) => session.mailbox.id !== activeSession.mailbox.id)
      setSessions(nextSessions)
      setActiveMailboxId(nextSessions[0]?.mailbox.id ?? null)
      setMessages([])
      setSelectedMessage(null)
      setSelectedMessageId(null)
      setNotice({ tone: "success", text: `邮箱 ${activeSession.mailbox.address} 已删除。` })
    } catch (error) {
      setNotice({ tone: "danger", text: (error as Error).message })
    } finally {
      setBusy(false)
    }
  }

  async function handleDeleteMessage() {
    if (!activeSession || !selectedMessage) return
    setBusy(true)

    try {
      await deleteMessage(activeSession.token, selectedMessage.id)
      setMessages((current) => current.filter((message) => message.id !== selectedMessage.id))
      setSelectedMessage(null)
      setSelectedMessageId(null)
      setNotice({ tone: "success", text: "邮件已从本地收件箱移除。" })
      await refreshMessages(false)
    } catch (error) {
      setNotice({ tone: "danger", text: (error as Error).message })
    } finally {
      setBusy(false)
    }
  }

  async function handleToggleSeen() {
    if (!activeSession || !selectedMessage) return
    setBusy(true)
    const nextSeen = !selectedMessage.seen

    try {
      const updated = await markMessageSeen(activeSession.token, selectedMessage.id, nextSeen)
      setSelectedMessage(updated)
      setMessages((current) =>
        current.map((item) => (item.id === updated.id ? { ...item, seen: nextSeen } : item)),
      )
    } catch (error) {
      setNotice({ tone: "danger", text: (error as Error).message })
    } finally {
      setBusy(false)
    }
  }

  async function handleDeliver(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (!activeSession) return
    setBusy(true)

    try {
      await deliverMessage({
        to: activeSession.mailbox.address,
        fromAddress: deliveryForm.fromAddress,
        fromName: deliveryForm.fromName,
        subject: deliveryForm.subject,
        text: deliveryForm.text,
      })
      setNotice({ tone: "success", text: "测试邮件已注入本地 inbound endpoint。" })
      await refreshMessages(false)
    } catch (error) {
      setNotice({ tone: "danger", text: (error as Error).message })
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="shell">
      <div className="backdrop backdrop-one" />
      <div className="backdrop backdrop-two" />

      <header className="hero">
        <div>
          <p className="eyebrow">KooixMail / Rust + React Rewrite</p>
          <h1>剥离 remote 关系，回到自管临时邮箱。</h1>
          <p className="hero-copy">
            现在的前端只面对本地 Rust API。账户、消息、SSE、投递闭环都在仓库里，不再依附 KooixMail API、
            Mail.tm、Mercure 或 provider 切换。
          </p>
        </div>

        <div className="hero-header-row">
          <div className="hero-card">
            <span className="badge">Backend</span>
            <strong>{API_BASE_URL}</strong>
            <p>默认域名由 `KOOIXMAIL_DOMAINS` 控制，邮件入口走 `/api/v1/inbound/messages`。</p>
          </div>
          <button
            className="theme-toggle"
            onClick={handleToggleTheme}
            type="button"
            aria-label={theme === "dark" ? "切换到浅色模式" : "切换到暗色模式"}
            title={theme === "dark" ? "切换到浅色模式" : "切换到暗色模式"}
          >
            <span className="theme-icon">
              {theme === "dark" ? (
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                  <circle cx="12" cy="12" r="5" />
                  <line x1="12" y1="1" x2="12" y2="3" />
                  <line x1="12" y1="21" x2="12" y2="23" />
                  <line x1="4.22" y1="4.22" x2="5.64" y2="5.64" />
                  <line x1="18.36" y1="18.36" x2="19.78" y2="19.78" />
                  <line x1="1" y1="12" x2="3" y2="12" />
                  <line x1="21" y1="12" x2="23" y2="12" />
                  <line x1="4.22" y1="19.78" x2="5.64" y2="18.36" />
                  <line x1="18.36" y1="5.64" x2="19.78" y2="4.22" />
                </svg>
              ) : (
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                  <path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z" />
                </svg>
              )}
            </span>
          </button>
        </div>
      </header>

      <section className="notice-row">
        <div className={`notice notice-${notice.tone}`} role="status" aria-live="polite">{notice.text}</div>
        <button className="ghost-button" onClick={() => void refreshMessages(true)} disabled={busy || !activeSession}>
          刷新收件箱
        </button>
      </section>

      <main className="grid">
        <section className="stack">
          <article className="panel">
            <div className="panel-header">
              <div>
                <p className="panel-kicker">铸邮箱</p>
                <h2>创建新的临时邮箱</h2>
              </div>
              <button
                className="ghost-button"
                onClick={() =>
                  setCreateForm((current) => ({
                    ...current,
                    username: createRandomToken(8),
                    password: createRandomToken(12),
                  }))
                }
                type="button"
              >
                重掷凭据
              </button>
            </div>

            <form className="form-grid" onSubmit={handleCreateMailbox}>
              <label>
                Username
                <input
                  value={createForm.username}
                  onChange={(event) => setCreateForm((current) => ({ ...current, username: event.target.value }))}
                />
              </label>
              <label>
                Domain
                <select
                  value={createForm.domain}
                  onChange={(event) => setCreateForm((current) => ({ ...current, domain: event.target.value }))}
                >
                  {domains.map((domain) => (
                    <option key={domain.id} value={domain.domain}>
                      {domain.domain}
                    </option>
                  ))}
                </select>
              </label>
              <label>
                Password
                <input
                  value={createForm.password}
                  onChange={(event) => setCreateForm((current) => ({ ...current, password: event.target.value }))}
                />
              </label>
              <label>
                保留时长
                <select
                  value={createForm.expiresIn}
                  onChange={(event) =>
                    setCreateForm((current) => ({ ...current, expiresIn: Number(event.target.value) }))
                  }
                >
                  {expiryOptions.map((option) => (
                    <option key={option.value} value={option.value}>
                      {option.label}
                    </option>
                  ))}
                </select>
              </label>
              <button className="primary-button" disabled={busy || !createForm.domain}>
                {busy ? "铸造中..." : "创建并登入"}
              </button>
            </form>

            {createdCredentials ? (
              <div className="credential-box">
                <strong>{createdCredentials.address}</strong>
                <span>{createdCredentials.password}</span>
                <button
                  className="ghost-button"
                  type="button"
                  onClick={() => {
                    void navigator.clipboard.writeText(
                      `${createdCredentials.address}\n${createdCredentials.password}`,
                    )
                    setNotice({ tone: "success", text: "凭据已复制到剪贴板。" })
                  }}
                >
                  复制凭据
                </button>
              </div>
            ) : null}
          </article>

          <article className="panel">
            <div className="panel-header">
              <div>
                <p className="panel-kicker">接管</p>
                <h2>登录已有邮箱</h2>
              </div>
            </div>

            <form className="form-grid" onSubmit={handleLogin}>
              <label>
                完整邮箱地址
                <input
                  placeholder="duck@kooixmail.local"
                  value={loginForm.address}
                  onChange={(event) => setLoginForm((current) => ({ ...current, address: event.target.value }))}
                />
              </label>
              <label>
                Password
                <input
                  type="password"
                  value={loginForm.password}
                  onChange={(event) => setLoginForm((current) => ({ ...current, password: event.target.value }))}
                />
              </label>
              <button className="secondary-button" disabled={busy}>
                登录
              </button>
            </form>
          </article>

          <article className="panel">
            <div className="panel-header">
              <div>
                <p className="panel-kicker">投递验证</p>
                <h2>向当前邮箱注入测试邮件</h2>
              </div>
            </div>

            <form className="form-grid" onSubmit={handleDeliver}>
              <label>
                当前收件地址
                <input value={activeSession?.mailbox.address ?? "请先创建或登录邮箱"} disabled />
              </label>
              <label>
                发件人地址
                <input
                  value={deliveryForm.fromAddress}
                  onChange={(event) =>
                    setDeliveryForm((current) => ({ ...current, fromAddress: event.target.value }))
                  }
                />
              </label>
              <label>
                发件人名称
                <input
                  value={deliveryForm.fromName}
                  onChange={(event) => setDeliveryForm((current) => ({ ...current, fromName: event.target.value }))}
                />
              </label>
              <label>
                Subject
                <input
                  value={deliveryForm.subject}
                  onChange={(event) => setDeliveryForm((current) => ({ ...current, subject: event.target.value }))}
                />
              </label>
              <label className="full-width">
                Text Body
                <textarea
                  rows={5}
                  value={deliveryForm.text}
                  onChange={(event) => setDeliveryForm((current) => ({ ...current, text: event.target.value }))}
                />
              </label>
              <button className="ghost-button" disabled={busy || !activeSession}>
                注入一封测试邮件
              </button>
            </form>
          </article>
        </section>

        <section className="panel sidebar-panel">
          <div className="panel-header">
            <div>
              <p className="panel-kicker">本地会话</p>
              <h2>已保存邮箱</h2>
            </div>
            <span className="count-pill" aria-label={`共 ${sessions.length} 个邮箱`}>{sessions.length}</span>
          </div>

          <div className="session-list">
            {sessions.length === 0 ? (
              <div className="empty-state">
                <strong>尚无保存的邮箱</strong>
                <p>请先创建或登录一个邮箱。</p>
              </div>
            ) : (
              sessions.map((session) => (
                <button
                  key={session.mailbox.id}
                  className={`session-card ${session.mailbox.id === activeMailboxId ? "session-card-active" : ""}`}
                  onClick={() => setActiveMailboxId(session.mailbox.id)}
                  type="button"
                >
                  <strong>{session.mailbox.address}</strong>
                  <span>创建于 {formatDate(session.mailbox.createdAt)}</span>
                  <span>过期于 {formatDate(session.mailbox.expiresAt)}</span>
                </button>
              ))
            )}
          </div>

          <button className="danger-button" disabled={!activeSession || busy} onClick={handleDeleteMailbox}>
            删除当前邮箱
          </button>
        </section>

        <section className="panel inbox-panel">
          <div className="panel-header">
            <div>
              <p className="panel-kicker">收件箱</p>
              <h2>{activeSession?.mailbox.address ?? "等待邮箱"}</h2>
            </div>
            <span className="count-pill" aria-label={`共 ${messages.length} 封邮件`}>{messages.length}</span>
          </div>

          <div className="message-list">
            {messages.length === 0 ? (
              <div className="empty-state">
                <strong>暂无邮件</strong>
                <p>创建邮箱后，可通过右侧投递表单或外部 SMTP/MTA 网关向 inbound endpoint 推送消息。</p>
              </div>
            ) : (
              messages.map((message) => (
                <button
                  key={message.id}
                  className={`message-card ${message.id === selectedMessageId ? "message-card-active" : ""}`}
                  onClick={() => setSelectedMessageId(message.id)}
                  type="button"
                >
                  <div className="message-card-head">
                    <strong>{message.subject}</strong>
                    <span>{formatRelative(message.createdAt)}</span>
                  </div>
                  <span className="message-from">{message.from.address}</span>
                  <p>{message.intro || "无正文预览"}</p>
                  <div className="message-meta">
                    <span>{message.seen ? "已读" : "未读"}</span>
                    <span>{message.size} bytes</span>
                  </div>
                </button>
              ))
            )}
          </div>

          {hasMore ? (
            <button className="ghost-button" onClick={handleLoadMore} disabled={busy} style={{ marginTop: "0.75rem" }}>
              加载更多邮件
            </button>
          ) : null}
        </section>

        <section className="panel detail-panel">
          <div className="panel-header">
            <div>
              <p className="panel-kicker">邮件详情</p>
              <h2>{selectedMessage?.subject ?? "选择一封邮件"}</h2>
            </div>
            <div className="detail-actions">
              <button
                className="ghost-button"
                disabled={!selectedMessage || busy}
                onClick={handleToggleSeen}
              >
                {selectedMessage?.seen ? "标为未读" : "标为已读"}
              </button>
              <button className="danger-button" disabled={!selectedMessage || busy} onClick={handleDeleteMessage}>
                删除邮件
              </button>
            </div>
          </div>

          {selectedMessage ? (
            <div className="detail-content">
              <div className="detail-meta">
                <span>From: {selectedMessage.from.name} &lt;{selectedMessage.from.address}&gt;</span>
                <span>To: {selectedMessage.to.map((item) => item.address).join(", ")}</span>
                <span>接收时间: {formatDate(selectedMessage.createdAt)}</span>
              </div>

              <article className="message-body">
                <pre>{selectedMessage.text || "该邮件没有 text body。"}</pre>
              </article>

              {selectedMessage.html.length > 0 ? (
                <article className="html-preview">
                  <h3>HTML 片段</h3>
                  <pre>{selectedMessage.html[0]}</pre>
                </article>
              ) : null}
            </div>
          ) : (
            <div className="empty-state">
              <strong>未选择邮件</strong>
              <p>左侧收件箱点击任意一封邮件，即可查看正文与元数据。</p>
            </div>
          )}
        </section>
      </main>
    </div>
  )
}

export default App
