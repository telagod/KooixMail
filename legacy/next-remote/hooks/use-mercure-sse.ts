"use client"

import { useEffect, useRef, useCallback, useState } from "react"
import { useAuth } from "@/contexts/auth-context"
import { getMercureToken } from "@/lib/api"
import type { Message } from "@/types"

interface UseMercureSSEOptions {
  onNewMessage?: (message: Message) => void
  onMessageUpdate?: (messageId: string, updates: Partial<Message>) => void
  onAccountUpdate?: (accountData: any) => void
  enabled?: boolean
}

export function useMercureSSE({
  onNewMessage,
  onMessageUpdate,
  onAccountUpdate,
  enabled = true,
}: UseMercureSSEOptions = {}) {
  const { currentAccount, token } = useAuth()
  const [isConnected, setIsConnected] = useState(false)
  const eventSourceRef = useRef<EventSource | null>(null)
  const callbacksRef = useRef({ onNewMessage, onMessageUpdate, onAccountUpdate })

  useEffect(() => {
    callbacksRef.current = { onNewMessage, onMessageUpdate, onAccountUpdate }
  }, [onNewMessage, onMessageUpdate, onAccountUpdate])

  useEffect(() => {
    let reconnectTimeout: NodeJS.Timeout | null = null
    let reconnectAttempts = 0

    const connect = async () => {
      if (!enabled || !currentAccount || !token) {
        return
      }

      if (eventSourceRef.current) {
        eventSourceRef.current.close()
      }

      try {
        const providerId = currentAccount.providerId || "duckmail"
        const presetProviders = [
          { id: "duckmail", mercureUrl: "https://mercure.duckmail.sbs/.well-known/mercure" },
          { id: "mailtm", mercureUrl: "https://mercure.mail.tm/.well-known/mercure" },
        ]
        const provider = presetProviders.find(p => p.id === providerId)
        if (!provider) {
          console.error(`❌ [Mercure] Provider configuration not found for: ${providerId}`)
          return
        }

        const { token: subscriptionToken } = await getMercureToken(token, provider.id)
        const mercureUrl = new URL(provider.mercureUrl)
        mercureUrl.searchParams.append("topic", `/accounts/${currentAccount.id}`)
        mercureUrl.searchParams.append("authorization", subscriptionToken)

        console.log("🔌 [Mercure] Connecting with native EventSource to:", mercureUrl.toString().replace(subscriptionToken, "SUB_TOKEN_HIDDEN"))
        
        const es = new EventSource(mercureUrl.toString())
        eventSourceRef.current = es

        es.onopen = () => {
          console.log("✅ [Mercure] Connected successfully to Mercure Hub")
          setIsConnected(true)
          reconnectAttempts = 0
        }

        const handleEvent = (event: MessageEvent) => {
          try {
            const data = JSON.parse(event.data)
            callbacksRef.current.onAccountUpdate?.(data)
          } catch (error) {
            // 忽略解析错误，因为可能是心跳信号
          }
        }

        es.addEventListener('account', handleEvent)
        es.onmessage = handleEvent

        es.onerror = () => {
          console.error("❌ [Mercure] Connection error.")
          setIsConnected(false)
          es.close()

          if (reconnectAttempts < 5) {
            const delay = Math.min(1000 * Math.pow(2, reconnectAttempts), 30000)
            console.log(`🔄 [Mercure] Reconnecting in ${delay / 1000}s...`)
            reconnectAttempts++
            reconnectTimeout = setTimeout(connect, delay)
          } else {
            console.error("❌ [Mercure] Max reconnection attempts reached.")
          }
        }
      } catch (error) {
        console.error("❌ [Mercure] Failed to establish SSE connection:", error)
      }
    }

    connect()

    return () => {
      if (reconnectTimeout) {
        clearTimeout(reconnectTimeout)
      }
      if (eventSourceRef.current) {
        eventSourceRef.current.close()
        eventSourceRef.current = null
      }
      setIsConnected(false)
    }
  }, [enabled, currentAccount?.id, token]) // 依赖项简化为关键变量

  // 外部控制的 connect 和 disconnect 保持可选
  const manualConnect = useCallback(() => {
    // 这个函数可以被外部调用，但主要的连接逻辑在 useEffect 中
    // 在这个实现中，我们让 useEffect 自动管理，所以这个函数可以为空
  }, [])

  const manualDisconnect = useCallback(() => {
    if (eventSourceRef.current) {
      eventSourceRef.current.close()
    }
  }, [])

  return {
    connect: manualConnect,
    disconnect: manualDisconnect,
    isConnected,
  }
}