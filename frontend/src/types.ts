export interface Domain {
  id: string
  domain: string
  isVerified: boolean
}

export interface Mailbox {
  id: string
  address: string
  createdAt: string
  expiresAt: string | null
}

export interface AuthSession {
  mailbox: Mailbox
  token: string
}

export interface Contact {
  name: string
  address: string
}

export interface Attachment {
  id: string
  filename: string
  contentType: string
  disposition: string
  size: number
  downloadUrl: string
}

export interface MessageSummary {
  id: string
  mailboxId: string
  from: Contact
  to: Contact[]
  subject: string
  intro: string
  seen: boolean
  hasAttachments: boolean
  size: number
  createdAt: string
  updatedAt: string
}

export interface MessageDetail extends MessageSummary {
  text: string
  html: string[]
  attachments: Attachment[]
}

export interface MailboxEvent {
  kind: string
  mailboxId: string
  messageId: string
  createdAt: string
}

export interface ApiErrorPayload {
  error: string
  message: string
}
