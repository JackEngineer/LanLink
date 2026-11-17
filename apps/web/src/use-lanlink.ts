import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect, useState } from 'react'
import { api } from './api'
import { errorMessage, type Transfer } from './lanlink'
import { useUi } from './ui-state'

export const agentKey = ['agent']
export const agentKeys = {
  health: [...agentKey, 'health'],
  device: [...agentKey, 'device'],
  peers: [...agentKey, 'peers'],
  pairings: [...agentKey, 'pairings'],
  settings: [...agentKey, 'settings'],
  transfers: [...agentKey, 'transfers'],
  messages: (peerId: string) => [...agentKey, 'messages', peerId],
}

export function useAgentQuery<T>(
  queryKey: readonly string[],
  queryFn: () => Promise<T>,
  refetchInterval: number | false,
  enabled = true,
) {
  return useQuery({ queryKey, queryFn, refetchInterval, enabled })
}

export type LiveConnection = 'connecting' | 'live' | 'reconnecting'

export function useAgentConnection(): LiveConnection {
  const client = useQueryClient()
  const [connection, setConnection] = useState<LiveConnection>('connecting')

  useEffect(() => {
    let disposed = false
    const controller = new AbortController()
    let socket: WebSocket | undefined
    let reconnectTimer: ReturnType<typeof setTimeout> | undefined
    let refreshTimer: ReturnType<typeof setTimeout> | undefined
    let delay = 500

    function refresh() {
      if (refreshTimer !== undefined) return
      refreshTimer = setTimeout(() => {
        refreshTimer = undefined
        void client.invalidateQueries({ queryKey: agentKey })
      }, 180)
    }

    function reconnect() {
      if (disposed) return
      setConnection('reconnecting')
      reconnectTimer = setTimeout(() => void connect(), delay)
      delay = Math.min(delay * 2, 5_000)
    }

    async function connect() {
      try {
        socket = await api.socket(controller.signal)
        if (disposed) {
          socket.close()
          return
        }
        socket.onopen = () => {
          setConnection('live')
          delay = 500
          void client.invalidateQueries({ queryKey: agentKey })
        }
        socket.onclose = reconnect
        socket.onmessage = (event: MessageEvent<unknown>) => {
          if (typeof event.data !== 'string') return
          try {
            const message: unknown = JSON.parse(event.data)
            if (
              message &&
              typeof message === 'object' &&
              'type' in message &&
              message.type === 'changed'
            ) {
              refresh()
            }
          } catch {
            // Ignore malformed notifications. HTTP snapshots remain authoritative.
          }
        }
      } catch {
        reconnect()
      }
    }

    void connect()
    return () => {
      disposed = true
      controller.abort()
      clearTimeout(reconnectTimer)
      clearTimeout(refreshTimer)
      if (socket) {
        socket.onopen = null
        socket.onclose = null
        socket.onmessage = null
        socket.close()
      }
    }
  }, [client])

  return connection
}

let activeUpload: { id: string; controller: AbortController } | undefined

export function useFileUploads() {
  const client = useQueryClient()

  async function processUploads() {
    if (activeUpload) return

    const { updateUpload, removeUpload } = useUi.getState()
    for (;;) {
      const next = useUi
        .getState()
        .uploads.find(({ status }) => status === 'queued')
      if (!next) return

      const { id, peerId, file } = next
      const controller = new AbortController()
      activeUpload = { id, controller }
      updateUpload(id, { status: 'uploading' })

      try {
        const transfer = await api.upload(
          peerId,
          file,
          (progress) => updateUpload(id, { progress }),
          controller.signal,
        )
        client.setQueryData<Transfer[]>(agentKeys.transfers, (current = []) => [
          ...current.filter(({ id }) => id !== transfer.id),
          transfer,
        ])
        removeUpload(id)
      } catch (error) {
        if (controller.signal.aborted) {
          removeUpload(id)
        } else {
          updateUpload(id, { status: 'failed', error: errorMessage(error) })
        }
      } finally {
        void client.invalidateQueries({ queryKey: agentKeys.transfers })
        activeUpload = undefined
      }
    }
  }

  function startUpload(
    peerId: string,
    file: File,
    id: string = crypto.randomUUID(),
  ) {
    const { uploads, putUpload } = useUi.getState()
    if (uploads.some((item) => item.id === id && item.status !== 'failed'))
      return

    putUpload({ id, peerId, file, progress: 0, status: 'queued' })
    void processUploads()
  }

  function dismissUpload(id: string) {
    if (activeUpload?.id === id) activeUpload.controller.abort()
    else useUi.getState().removeUpload(id)
  }

  return { startUpload, dismissUpload }
}
