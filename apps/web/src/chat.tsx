import { useMutation, useQueryClient } from '@tanstack/react-query'
import {
  CircleHelp,
  Link2,
  MessageSquare,
  Paperclip,
  Send,
  Upload,
} from 'lucide-react'
import {
  type DragEvent,
  type FormEvent,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from 'react'
import { api } from './api'
import { EmptyState, InlineError } from './components'
import {
  MESSAGE_MAX_BYTES,
  formatBytes,
  formatTime,
  isoTime,
  messageLabels,
  type Message,
  type Pairing,
  type Peer,
} from './lanlink'
import { UploadRows } from './transfers'
import { agentKey, agentKeys, useFileUploads } from './use-lanlink'
import { useUi } from './ui-state'

export function StartChat() {
  const openSidebar = useUi((state) => state.setSidebarOpen)

  return (
    <EmptyState icon={<MessageSquare size={29} />} title="选择设备开始聊天">
      <p>在设备列表中选中一台设备，配对后即可互传消息和文件。</p>
      <button
        className="button mobile-only"
        type="button"
        onClick={() => openSidebar(true)}
      >
        查看设备
      </button>
    </EmptyState>
  )
}

export function PairPeer({
  peer,
  pairing,
  agentAvailable,
}: {
  peer: Peer
  pairing: Pairing | undefined
  agentAvailable: boolean
}) {
  const client = useQueryClient()
  const showPairing = useUi((state) => state.setPairingId)
  const pair = useMutation({
    mutationFn: () => api.pair(peer.id),
    onSuccess: ({ id }) => {
      showPairing(id)
      void client.invalidateQueries({ queryKey: agentKey })
    },
  })

  return (
    <EmptyState icon={<Link2 size={29} />} title="先配对此设备">
      <p>在两台设备上核对同一个安全码，再分别确认配对。</p>
      {pairing ? (
        <button
          className="button button-primary"
          type="button"
          onClick={() => showPairing(pairing.id)}
        >
          查看安全码
        </button>
      ) : (
        <button
          className="button button-primary"
          type="button"
          disabled={!agentAvailable || !peer.online || pair.isPending}
          onClick={() => pair.mutate()}
        >
          {pair.isPending ? '正在发起…' : '发起配对'}
        </button>
      )}
      {!peer.online && <p>设备已离线。</p>}
      {pair.isSuccess && !pairing && <p role="status">配对请求已发出。</p>}
      <InlineError error={pair.error} />
    </EmptyState>
  )
}

interface ChatProps {
  peer: Peer
  messages: Message[]
  maxFileSize: number | undefined
  agentAvailable: boolean
  isLoading: boolean
  messagesError: Error | null
}

export function Chat({
  peer,
  messages,
  maxFileSize,
  agentAvailable,
  isLoading,
  messagesError,
}: ChatProps) {
  const client = useQueryClient()
  const inputFile = useRef<HTMLInputElement>(null)
  const inputMessage = useRef<HTMLTextAreaElement>(null)
  const scrollArea = useRef<HTMLDivElement>(null)
  const pinnedToBottom = useRef(true)
  const dragDepth = useRef(0)
  const [dragging, setDragging] = useState(false)
  const [fileError, setFileError] = useState<Error | null>(null)
  const draft = useUi((state) => state.drafts[peer.id] ?? '')
  const updateDraft = useUi((state) => state.setDraft)
  const { startUpload } = useFileUploads()
  const canAttach = agentAvailable && maxFileSize !== undefined
  const hasText = draft.trim().length > 0
  const tooLong =
    new TextEncoder().encode(draft.trim()).length > MESSAGE_MAX_BYTES

  const send = useMutation({
    mutationFn: (body: string) => api.send(peer.id, body),
    onSuccess: (message, body) => {
      if (useUi.getState().drafts[peer.id]?.trim() === body)
        updateDraft(peer.id, '')
      pinnedToBottom.current = true
      client.setQueryData<Message[]>(
        agentKeys.messages(peer.id),
        (current = []) => [
          ...current.filter(({ id }) => id !== message.id),
          message,
        ],
      )
      void client.invalidateQueries({ queryKey: agentKeys.messages(peer.id) })
    },
  })

  useLayoutEffect(() => {
    const container = scrollArea.current
    if (container && pinnedToBottom.current) {
      container.scrollTop = container.scrollHeight
    }
  }, [messages])

  function sendMessage(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (hasText && !tooLong && agentAvailable && !send.isPending) {
      send.mutate(draft.trim())
      inputMessage.current?.focus()
    }
  }

  function uploadFiles(files: FileList) {
    if (!maxFileSize || !agentAvailable) {
      setFileError(new Error('无法读取本地文件上传设置，请稍后重试。'))
      return
    }

    const oversized: string[] = []
    for (const file of files) {
      if (file.size > maxFileSize) oversized.push(file.name)
      else void startUpload(peer.id, file)
    }
    if (files.length > 0 && oversized.length === 0) {
      useUi.getState().setView('transfers')
    }
    setFileError(
      oversized.length
        ? new Error(
            `${oversized.join('、')} 超过 ${formatBytes(maxFileSize)} 的单文件上限。`,
          )
        : null,
    )
  }

  function dragEnter(event: DragEvent<HTMLElement>) {
    if (!event.dataTransfer.types.includes('Files')) return
    event.preventDefault()
    dragDepth.current += 1
    setDragging(true)
  }

  function dragLeave(event: DragEvent<HTMLElement>) {
    if (!event.dataTransfer.types.includes('Files')) return
    dragDepth.current = Math.max(0, dragDepth.current - 1)
    if (dragDepth.current === 0) setDragging(false)
  }

  function drop(event: DragEvent<HTMLElement>) {
    if (!event.dataTransfer.types.includes('Files')) return
    event.preventDefault()
    dragDepth.current = 0
    setDragging(false)
    if (event.dataTransfer.files.length > 0)
      uploadFiles(event.dataTransfer.files)
  }

  const sorted = useMemo(
    () =>
      [...messages].sort((left, right) => left.created_at - right.created_at),
    [messages],
  )

  return (
    <section
      className="chat"
      aria-label={`与 ${peer.name} 的聊天`}
      onDragEnter={dragEnter}
      onDragLeave={dragLeave}
      onDragOver={(event) => {
        if (event.dataTransfer.types.includes('Files')) {
          event.preventDefault()
          event.dataTransfer.dropEffect = canAttach ? 'copy' : 'none'
        }
      }}
      onDrop={drop}
    >
      <div
        className="chat-history"
        ref={scrollArea}
        onScroll={(event) => {
          const { scrollHeight, scrollTop, clientHeight } = event.currentTarget
          pinnedToBottom.current = scrollHeight - scrollTop - clientHeight < 96
        }}
      >
        <InlineError error={messagesError} />
        {!messagesError && isLoading && (
          <p className="loading">正在读取聊天记录…</p>
        )}
        {!messagesError && !isLoading && sorted.length === 0 && (
          <EmptyState icon={<MessageSquare size={29} />} title="还没有消息">
            <p>发送一条消息，或拖入文件。</p>
          </EmptyState>
        )}
        {sorted.length > 0 && (
          <ol
            className="messages"
            aria-label="聊天记录"
            aria-live="polite"
            aria-relevant="additions"
          >
            {sorted.map((message) => (
              <MessageBubble
                key={message.id}
                message={message}
                agentAvailable={agentAvailable}
              />
            ))}
          </ol>
        )}
      </div>
      <div className="composer-area">
        <UploadRows peerId={peer.id} agentAvailable={agentAvailable} />
        <InlineError error={fileError || send.error} />
        {!peer.online && agentAvailable && (
          <p className="composer-note">
            <CircleHelp size={15} aria-hidden="true" />{' '}
            设备离线。消息将在对方重新连接后继续发送。
          </p>
        )}
        <form className="composer" onSubmit={sendMessage}>
          <input
            type="file"
            multiple
            hidden
            ref={inputFile}
            onChange={(event) => {
              if (event.currentTarget.files)
                uploadFiles(event.currentTarget.files)
              event.currentTarget.value = ''
            }}
          />
          <button
            className="icon-button attach-button"
            type="button"
            aria-label="选择文件"
            title="选择文件"
            disabled={!canAttach}
            onClick={() => inputFile.current?.click()}
          >
            <Paperclip size={20} aria-hidden="true" />
          </button>
          <textarea
            ref={inputMessage}
            value={draft}
            aria-label="输入消息"
            aria-describedby={tooLong ? 'message-limit' : undefined}
            aria-invalid={tooLong || undefined}
            placeholder="输入消息…"
            onChange={(event) => updateDraft(peer.id, event.target.value)}
            onKeyDown={(event) => {
              if (
                event.key === 'Enter' &&
                !event.shiftKey &&
                !event.nativeEvent.isComposing &&
                event.nativeEvent.keyCode !== 229
              ) {
                event.preventDefault()
                event.currentTarget.form?.requestSubmit()
              }
            }}
          />
          <button
            className="button button-primary send-button"
            type="submit"
            disabled={!hasText || tooLong || !agentAvailable || send.isPending}
          >
            <Send size={16} aria-hidden="true" /> 发送
          </button>
        </form>
        {tooLong && (
          <p
            id="message-limit"
            className="composer-note text-error"
            role="alert"
          >
            单条消息最多 16 KiB。
          </p>
        )}
      </div>
      {dragging && (
        <div className="drop-overlay" aria-hidden="true">
          <Upload size={32} />
          <strong>{canAttach ? '松开以上传文件' : '本地服务暂不可用'}</strong>
        </div>
      )}
    </section>
  )
}

function MessageBubble({
  message,
  agentAvailable,
}: {
  message: Message
  agentAvailable: boolean
}) {
  const client = useQueryClient()
  const retry = useMutation({
    mutationFn: () => api.retryMessage(message.id),
    onSettled: () =>
      void client.invalidateQueries({
        queryKey: agentKeys.messages(message.peer_id),
      }),
  })
  const outgoing = message.direction === 'outgoing'

  return (
    <li className={outgoing ? 'message message-outgoing' : 'message'}>
      <div className="message-bubble">{message.body}</div>
      <div className="message-meta">
        <time dateTime={isoTime(message.created_at)}>
          {formatTime(message.created_at)}
        </time>
        {outgoing && (
          <span className={message.status === 'failed' ? 'text-error' : ''}>
            {messageLabels[message.status]}
          </span>
        )}
        {outgoing && message.status === 'failed' && (
          <button
            className="text-button"
            type="button"
            disabled={retry.isPending || !agentAvailable}
            onClick={() => retry.mutate()}
          >
            重试
          </button>
        )}
      </div>
      <InlineError error={retry.error} />
    </li>
  )
}
