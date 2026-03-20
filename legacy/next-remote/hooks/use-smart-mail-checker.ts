"use client"

import { useCallback, useRef, useState, useEffect } from "react"
import { useMercureSSE } from "./use-mercure-sse"
import { useMailChecker } from "./use-mail-checker"
import { getMessages } from "@/lib/api"
import { useAuth } from "@/contexts/auth-context"
import type { Message } from "@/types"

interface UseSmartMailCheckerOptions {
  onNewMessage?: (message: Message) => void
  onMessagesUpdate?: (messages: Message[]) => void
  enabled?: boolean
}

export function useSmartMailChecker({
  onNewMessage,
  onMessagesUpdate,
  enabled = true, // 这个参数只控制轮询策略，不影响 Mercure
}: UseSmartMailCheckerOptions = {}) {
  const { token, currentAccount } = useAuth()
  const lastUsedRef = useRef<number>(0)
  const isRefreshingRef = useRef(false)
  const [mercureConnected, setMercureConnected] = useState(false)
  const [mercureAttempted, setMercureAttempted] = useState(false)

  // 当 Mercure 检测到更新时，刷新消息列表
  const handleAccountUpdate = useCallback(async (accountData: any) => {
    if (!token || isRefreshingRef.current) return

    console.log(`📧 [SmartChecker] Mercure update detected, refreshing messages...`)

    isRefreshingRef.current = true
    try {
      // 获取最新的消息列表
      const providerId = currentAccount?.providerId || "duckmail"
      const { messages } = await getMessages(token, 1, providerId)
      const currentMessages = messages || []

      // 更新消息列表
      onMessagesUpdate?.(currentMessages)

      console.log(`✅ [SmartChecker] Refreshed messages, found ${currentMessages.length} total`)
    } catch (error) {
      console.error("❌ [SmartChecker] Failed to refresh messages:", error)
    } finally {
      isRefreshingRef.current = false
    }
  }, [token, onMessagesUpdate])

  // 处理直接收到的新消息
  const handleNewMessage = useCallback((message: any) => {
    console.log(`📧 [SmartChecker] New message received directly:`, message.subject)
    onNewMessage?.(message)
    // 也触发消息列表刷新
    handleAccountUpdate({ used: Date.now() })
  }, [onNewMessage, handleAccountUpdate])

  // 尝试使用 Mercure SSE - 总是尝试连接，不受 enabled 参数控制
  const mercureResult = useMercureSSE({
    onNewMessage: handleNewMessage,
    onAccountUpdate: handleAccountUpdate,
    enabled: true, // Mercure 总是尝试连接
  })

  // 监听 Mercure 连接状态变化，使用稳定的状态更新
  useEffect(() => {
    const isConnected = mercureResult.isConnected

    if (isConnected !== mercureConnected) {
      setMercureConnected(isConnected)
      setMercureAttempted(true)

      if (isConnected) {
        console.log("🚀 [SmartChecker] Mercure connected - using real-time updates")
      } else if (mercureAttempted) {
        console.log("🔄 [SmartChecker] Mercure disconnected - falling back to polling")
      }
    }
  }, [mercureResult.isConnected, mercureConnected, mercureAttempted])

  // 备用轮询策略：
  // 1. 只有在 Mercure 连接失败时才考虑启用
  // 2. 用户可以通过 enabled 参数控制是否启用轮询
  const shouldUsePolling = enabled && mercureAttempted && !mercureConnected

  const pollingResult = useMailChecker({
    onNewMessage,
    onMessagesUpdate,
    interval: 30000, // 30秒备用轮询，频率较低
    enabled: shouldUsePolling,
  })

  return {
    isUsingMercure: mercureConnected,
    isUsingPolling: shouldUsePolling,
    mercureAttempted,
    mercureConnect: mercureResult.connect,
    mercureDisconnect: mercureResult.disconnect,
  }
}
