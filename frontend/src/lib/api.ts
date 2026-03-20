import type {
  ApiErrorPayload,
  AuthSession,
  Domain,
  MailboxEvent,
  MessageDetail,
  MessageSummary,
} from "../types"

const API_BASE_URL = (import.meta.env.VITE_API_BASE_URL as string | undefined)?.replace(/\/$/, "") ??
  "http://127.0.0.1:3000/api/v1"

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(`${API_BASE_URL}${path}`, {
    ...init,
    headers: {
      "Content-Type": "application/json",
      ...(init?.headers ?? {}),
    },
  })

  if (!response.ok) {
    let payload: ApiErrorPayload | null = null
    try {
      payload = (await response.json()) as ApiErrorPayload
    } catch {
      payload = null
    }

    throw new Error(payload?.message ?? `request failed: ${response.status}`)
  }

  if (response.status === 204) {
    return undefined as T
  }

  return response.json() as Promise<T>
}

export function listDomains() {
  return request<Domain[]>("/domains")
}

export function createMailbox(payload: { address: string; password: string; expiresIn?: number }) {
  return request<AuthSession>("/mailboxes", {
    method: "POST",
    body: JSON.stringify(payload),
  })
}

export function createSession(payload: { address: string; password: string }) {
  return request<AuthSession>("/sessions", {
    method: "POST",
    body: JSON.stringify(payload),
  })
}

export function getCurrentMailbox(token: string) {
  return request<AuthSession["mailbox"]>("/me", {
    headers: { Authorization: `Bearer ${token}` },
  })
}

export function listMessages(token: string, limit = 50, offset = 0) {
  const params = new URLSearchParams()
  if (limit !== 50) params.set("limit", String(limit))
  if (offset > 0) params.set("offset", String(offset))
  const query = params.toString()
  return request<MessageSummary[]>(`/messages${query ? `?${query}` : ""}`, {
    headers: { Authorization: `Bearer ${token}` },
  })
}

export function getMessage(token: string, messageId: string) {
  return request<MessageDetail>(`/messages/${messageId}`, {
    headers: { Authorization: `Bearer ${token}` },
  })
}

export function markMessageSeen(token: string, messageId: string, seen: boolean) {
  return request<MessageDetail>(`/messages/${messageId}`, {
    method: "PATCH",
    headers: { Authorization: `Bearer ${token}` },
    body: JSON.stringify({ seen }),
  })
}

export function deleteMessage(token: string, messageId: string) {
  return request<void>(`/messages/${messageId}`, {
    method: "DELETE",
    headers: { Authorization: `Bearer ${token}` },
  })
}

export function deleteMailbox(token: string, mailboxId: string) {
  return request<void>(`/mailboxes/${mailboxId}`, {
    method: "DELETE",
    headers: { Authorization: `Bearer ${token}` },
  })
}

export function deliverMessage(payload: {
  to: string
  fromAddress: string
  fromName?: string
  subject?: string
  text?: string
  html?: string
}) {
  return request<MessageDetail>("/inbound/messages", {
    method: "POST",
    body: JSON.stringify(payload),
  })
}

export function subscribeMailboxEvents(
  mailboxId: string,
  token: string,
  onEvent: (event: MailboxEvent) => void,
) {
  const url = new URL(`${API_BASE_URL}/events`)
  url.searchParams.set("mailboxId", mailboxId)
  url.searchParams.set("token", token)

  const source = new EventSource(url)
  const handler = (event: MessageEvent<string>) => {
    try {
      onEvent(JSON.parse(event.data) as MailboxEvent)
    } catch {
      // ignore keep-alive noise
    }
  }

  source.onmessage = handler
  source.addEventListener("message.created", handler as EventListener)
  source.addEventListener("message.updated", handler as EventListener)
  source.addEventListener("message.deleted", handler as EventListener)
  return source
}

export { API_BASE_URL }
