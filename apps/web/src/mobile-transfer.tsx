import { QRCodeSVG } from '@rc-component/qrcode'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { ArrowDownToLine, ArrowUpFromLine, Wifi } from 'lucide-react'
import { useEffect, useRef, useState, type FormEvent } from 'react'
import { api } from './api'
import { InlineError, Modal } from './components'
import { formatBytes, type MobileSession } from './lanlink'

const mobileKey = ['agent', 'mobile']
const labels = {
  inviting: '等待扫码',
  pending: '等待电脑确认',
  approved: '设备已连接',
}

type SessionAction =
  | { type: 'start'; address: string }
  | { type: 'approve'; code: string }
  | { type: 'end' }

function updateSession(action: SessionAction) {
  switch (action.type) {
    case 'start':
      return api.startMobile(action.address)
    case 'approve':
      return api.approveMobile(action.code)
    case 'end':
      return api.endMobile()
  }
}

export function MobileTransferDialog({ onClose }: { onClose: () => void }) {
  const client = useQueryClient()
  const status = useQuery({
    queryKey: mobileKey,
    queryFn: ({ signal }) => api.mobile(signal),
    refetchInterval: 1_500,
    retry: false,
  })
  const [selectedAddress, setSelectedAddress] = useState('')
  const addresses = status.data?.addresses ?? []
  const address = addresses.includes(selectedAddress)
    ? selectedAddress
    : (addresses[0] ?? '')
  const session = status.data?.session
  const action = useMutation({
    mutationFn: updateSession,
    onSuccess: async (data) => {
      await client.cancelQueries({ queryKey: mobileKey })
      client.setQueryData(mobileKey, data)
    },
    onSettled: () => void client.invalidateQueries({ queryKey: mobileKey }),
  })

  return (
    <Modal title="手机互传" onClose={onClose}>
      <p className="mobile-network">
        <Wifi size={17} aria-hidden="true" /> 请让手机或平板和电脑连接同一个
        Wi-Fi。
      </p>
      <p className="mobile-warning">传输未加密，仅在可信 Wi-Fi 使用。</p>
      <InlineError error={status.error} />
      {status.isPending && <p role="status">正在读取互传状态…</p>}
      {!session && status.data && (
        <form
          className="mobile-start"
          onSubmit={(event) => {
            event.preventDefault()
            if (address && !action.isPending) {
              action.mutate({ type: 'start', address })
            }
          }}
        >
          <p className="modal-copy" role="status">
            {action.isSuccess && action.variables?.type === 'end'
              ? '互传已结束。'
              : '生成二维码后，用手机或平板扫码连接。'}
          </p>
          {addresses.length > 1 && (
            <label className="mobile-field">
              电脑在当前 Wi-Fi 的 IPv4 地址
              <select
                value={address}
                onChange={(event) => setSelectedAddress(event.target.value)}
              >
                {addresses.map((item) => (
                  <option key={item} value={item}>
                    {item}
                  </option>
                ))}
              </select>
            </label>
          )}
          {addresses.length === 0 && (
            <p className="modal-detail">
              未找到局域网 IPv4 地址，请检查电脑的网络连接。
            </p>
          )}
          <div className="modal-actions">
            <button
              className="button button-primary"
              type="submit"
              disabled={!address || action.isPending || status.isError}
            >
              {action.isPending ? '正在生成…' : '生成二维码'}
            </button>
          </div>
        </form>
      )}
      {session && (
        <>
          <div className="mobile-session-status">
            <span role="status">{labels[session.phase]}</span>
            <SessionTime expiresAt={session.expires_at} />
          </div>
          {session.phase === 'inviting' && (
            <div className="mobile-invite">
              <QRCodeSVG
                value={session.url}
                size={216}
                level="M"
                marginSize={4}
                role="img"
                aria-label="手机互传配对二维码"
              />
              <p className="modal-copy">用手机或平板的相机扫描二维码。</p>
            </div>
          )}
          {session.phase === 'pending' && (
            <ApproveMobile
              key={session.url}
              disabled={action.isPending || status.isError}
              onApprove={(code) => action.mutate({ type: 'approve', code })}
            />
          )}
          {session.phase === 'approved' && (
            <MobileFiles
              key={session.url}
              session={session}
              disabled={action.isPending || status.isError}
            />
          )}
          <div className="modal-actions">
            <button
              className="button"
              type="button"
              disabled={action.isPending}
              onClick={() => action.mutate({ type: 'end' })}
            >
              {session.phase === 'pending' ? '拒绝连接' : '结束互传'}
            </button>
          </div>
        </>
      )}
      <InlineError error={action.error} />
    </Modal>
  )
}

function SessionTime({ expiresAt }: { expiresAt: number }) {
  const [now, setNow] = useState(Date.now)
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 500)
    return () => window.clearInterval(timer)
  }, [])

  const seconds = Math.max(0, Math.ceil((expiresAt - now) / 1_000))
  if (!seconds) return <span>正在更新会话状态…</span>
  const minutes = Math.floor(seconds / 60)
  const remainder = String(seconds % 60).padStart(2, '0')
  return (
    <span className="mobile-timer">
      剩余 {minutes}:{remainder}
    </span>
  )
}

function ApproveMobile({
  disabled,
  onApprove,
}: {
  disabled: boolean
  onApprove: (code: string) => void
}) {
  const [code, setCode] = useState('')

  return (
    <form
      className="mobile-approve"
      onSubmit={(event) => {
        event.preventDefault()
        if (!disabled && code.length === 6) onApprove(code)
      }}
    >
      <p className="modal-copy">
        只允许你正在操作的设备连接。请输入该设备页面显示的 6 位确认码。
      </p>
      <label className="mobile-field">
        手机确认码
        <input
          className="mobile-code-input"
          type="text"
          inputMode="numeric"
          autoComplete="off"
          maxLength={6}
          value={code}
          onChange={(event) => setCode(event.target.value.replace(/\D/g, ''))}
        />
      </label>
      <button
        className="button button-primary"
        type="submit"
        disabled={disabled || code.length !== 6}
      >
        允许连接
      </button>
    </form>
  )
}

function MobileFiles({
  session,
  disabled,
}: {
  session: MobileSession
  disabled: boolean
}) {
  const client = useQueryClient()
  const picker = useRef<HTMLInputElement>(null)
  const upload = useRef<AbortController | null>(null)
  const [file, setFile] = useState<File | null>(null)
  const [progress, setProgress] = useState<number | null>(null)
  const [failure, setFailure] = useState<Error | null>(null)
  const [sent, setSent] = useState('')
  const limitError =
    file && file.size > session.max_file_size
      ? new Error(`文件大小超过 ${formatBytes(session.max_file_size)}。`)
      : null

  useEffect(() => () => upload.current?.abort(), [])

  async function send(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    if (!file || limitError || disabled || upload.current) return

    const controller = new AbortController()
    upload.current = controller
    setProgress(0)
    setFailure(null)
    setSent('')
    try {
      const result = await api.uploadMobile(
        file,
        setProgress,
        controller.signal,
      )
      setSent(`已发送 ${result.name}`)
      setFile(null)
      if (picker.current) picker.current.value = ''
      void client.invalidateQueries({ queryKey: mobileKey })
    } catch (error) {
      if (controller.signal.aborted) return
      setFailure(error instanceof Error ? error : new Error('文件发送失败。'))
    } finally {
      upload.current = null
      if (!controller.signal.aborted) setProgress(null)
    }
  }

  return (
    <div className="mobile-files">
      <form className="mobile-send" onSubmit={(event) => void send(event)}>
        <label className="mobile-field">
          向手机发送文件
          <input
            ref={picker}
            type="file"
            disabled={disabled || progress !== null}
            onChange={(event) => {
              setFile(event.target.files?.[0] ?? null)
              setFailure(null)
              setSent('')
            }}
          />
        </label>
        <p className="modal-copy">
          单个文件最大 {formatBytes(session.max_file_size)}
        </p>
        <button
          className="button button-primary"
          type="submit"
          disabled={
            disabled || !file || Boolean(limitError) || progress !== null
          }
        >
          <ArrowUpFromLine size={16} aria-hidden="true" />
          {progress === null ? '发送文件' : '正在发送…'}
        </button>
        {progress !== null && (
          <div className="mobile-progress">
            <progress value={progress} max={100} aria-label="文件上传进度" />
            <span>{progress}%</span>
          </div>
        )}
        <InlineError error={failure || limitError} />
        <p className="mobile-sent" role="status">
          {sent}
        </p>
      </form>
      <div className="mobile-inbox">
        <h3>手机发来的文件</h3>
        {session.files.every((file) => file.direction !== 'to_computer') ? (
          <p className="modal-copy">暂无文件。</p>
        ) : (
          <ul aria-label="手机发来的文件">
            {session.files
              .filter((file) => file.direction === 'to_computer')
              .map((file) => (
                <li key={file.id}>
                  <span className="mobile-file-name">
                    {file.name}
                    <small>{formatBytes(file.size)}</small>
                  </span>
                  <a
                    className="button button-small"
                    href={api.mobileDownloadUrl(file.id)}
                    referrerPolicy="same-origin"
                    download={file.name}
                    aria-label={`下载 ${file.name}`}
                  >
                    <ArrowDownToLine size={15} aria-hidden="true" /> 下载
                  </a>
                </li>
              ))}
          </ul>
        )}
      </div>
    </div>
  )
}
