import type { AuthSession } from "../types"

const SESSIONS_KEY = "kooixmail.local.sessions"
const ACTIVE_KEY = "kooixmail.local.active"

export function loadSessions() {
  try {
    const raw = localStorage.getItem(SESSIONS_KEY)
    if (!raw) return []
    return JSON.parse(raw) as AuthSession[]
  } catch {
    return []
  }
}

export function saveSessions(sessions: AuthSession[]) {
  localStorage.setItem(SESSIONS_KEY, JSON.stringify(sessions))
}

export function loadActiveMailboxId() {
  return localStorage.getItem(ACTIVE_KEY)
}

export function saveActiveMailboxId(mailboxId: string | null) {
  if (mailboxId) {
    localStorage.setItem(ACTIVE_KEY, mailboxId)
    return
  }

  localStorage.removeItem(ACTIVE_KEY)
}
