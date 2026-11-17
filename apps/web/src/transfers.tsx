import { useMutation, useQueryClient } from '@tanstack/react-query'
import {
  ArrowDownToLine,
  ArrowLeftRight,
  ArrowUpFromLine,
  File,
  X,
} from 'lucide-react'
import { useState } from 'react'
import { useShallow } from 'zustand/react/shallow'
import { api } from './api'
import { EmptyState, InlineError } from './components'
import {
  formatBytes,
  formatTime,
  isoTime,
  percent,
  transferActions,
  transferLabels,
  type Direction,
  type Peer,
  type Transfer,
  type TransferAction,
} from './lanlink'
import { agentKeys, useFileUploads } from './use-lanlink'
import { useUi } from './ui-state'

interface TransfersProps {
  records: Transfer[]
  peers: Peer[]
  agentAvailable: boolean
  isLoading: boolean
  error: Error | null
}

const filters = [
  { id: 'all', label: '全部' },
  { id: 'incoming', label: '接收' },
  { id: 'outgoing', label: '发送' },
] as const

export function Transfers({
  records,
  peers,
  agentAvailable,
  isLoading,
  error,
}: TransfersProps) {
  const [filter, setFilter] = useState<Direction | 'all'>('all')
  const names = new Map(peers.map((peer) => [peer.id, peer.name]))
  const hasUploads = useUi((state) => state.uploads.length > 0)
  const visible = records
    .filter((transfer) => filter === 'all' || transfer.direction === filter)
    .sort((left, right) => {
      const leftPending =
        left.status === 'offered' && left.direction === 'incoming'
      const rightPending =
        right.status === 'offered' && right.direction === 'incoming'
      return (
        Number(rightPending) - Number(leftPending) ||
        right.created_at - left.created_at
      )
    })
  const showUploads = filter !== 'incoming' && hasUploads

  return (
    <section className="transfers content-scroll" aria-label="文件传输记录">
      <div className="content-inner">
        <div className="segmented" aria-label="筛选传输记录">
          {filters.map(({ id, label }) => (
            <button
              type="button"
              aria-pressed={filter === id}
              key={id}
              onClick={() => setFilter(id)}
            >
              {label}
            </button>
          ))}
        </div>
        <InlineError error={error} />
        {showUploads && <UploadRows agentAvailable={agentAvailable} />}
        {!error && isLoading && <p className="loading">正在读取传输记录…</p>}
        {!error && !isLoading && !showUploads && visible.length === 0 && (
          <EmptyState
            icon={<ArrowLeftRight size={29} />}
            title={filter === 'all' ? '暂无传输记录' : '没有符合条件的传输'}
          >
            {filter === 'all' && <p>从聊天中发送文件，或在这里接收文件。</p>}
          </EmptyState>
        )}
        {visible.length > 0 && (
          <ol className="transfer-list">
            {visible.map((transfer) => (
              <TransferRow
                key={transfer.id}
                transfer={transfer}
                peerName={
                  names.get(transfer.peer_id) ||
                  `设备 ${transfer.peer_id.slice(0, 8)}`
                }
                agentAvailable={agentAvailable}
              />
            ))}
          </ol>
        )}
      </div>
    </section>
  )
}

const actionLabels: Record<TransferAction, string> = {
  accept: '接收',
  reject: '拒绝',
  cancel: '取消',
  retry: '重试',
}

function TransferRow({
  transfer,
  peerName,
  agentAvailable,
}: {
  transfer: Transfer
  peerName: string
  agentAvailable: boolean
}) {
  const client = useQueryClient()
  const perform = useMutation({
    mutationFn: (action: TransferAction) =>
      api.transferAction(transfer.id, action),
    onSettled: () =>
      void client.invalidateQueries({ queryKey: agentKeys.transfers }),
  })
  const actions = transferActions(transfer)
  const incoming = transfer.direction === 'incoming'
  const DirectionIcon = incoming ? ArrowDownToLine : ArrowUpFromLine
  const inProgress =
    transfer.status === 'transferring' || transfer.status === 'paused'
  const downloadable = incoming && transfer.status === 'completed'

  return (
    <li className="transfer-row">
      <span className="file-avatar">
        <DirectionIcon size={20} aria-hidden="true" />
      </span>
      <div className="transfer-content">
        <h2 className="file-name" title={transfer.name}>
          {transfer.name}
        </h2>
        <div className="file-details">
          <span className="min-w-0 truncate" title={peerName}>
            {incoming ? '来自' : '发送至'} {peerName}
          </span>
          <span>{formatBytes(transfer.size)}</span>
          <time dateTime={isoTime(transfer.created_at)}>
            {formatTime(transfer.created_at)}
          </time>
        </div>
        {inProgress && (
          <FileProgress
            value={percent(transfer.offset, transfer.size)}
            label={`${transfer.name} 传输进度`}
            detail={`${formatBytes(transfer.offset)} / ${formatBytes(transfer.size)}`}
          />
        )}
        {transfer.error && <p className="file-error">{transfer.error}</p>}
        <InlineError error={perform.error} />
      </div>
      <div className="transfer-actions">
        <span className={`transfer-status status-${transfer.status}`}>
          {transferLabels[transfer.status]}
        </span>
        <div className="file-actions">
          {downloadable && agentAvailable && (
            <a
              className="button button-small"
              href={api.downloadUrl(transfer.id)}
              download={transfer.name}
              aria-label={`下载 ${transfer.name}`}
            >
              <ArrowDownToLine size={15} aria-hidden="true" /> 下载
            </a>
          )}
          {actions.map((action) => (
            <button
              className={
                action === 'accept'
                  ? 'button button-small button-primary'
                  : 'button button-small'
              }
              type="button"
              key={action}
              disabled={perform.isPending || !agentAvailable}
              aria-label={`${actionLabels[action]} ${transfer.name}`}
              onClick={() => perform.mutate(action)}
            >
              {actionLabels[action]}
            </button>
          ))}
        </div>
      </div>
    </li>
  )
}

export function UploadRows({
  peerId,
  agentAvailable,
}: {
  peerId?: string
  agentAvailable: boolean
}) {
  const uploads = useUi(
    useShallow((state) =>
      state.uploads.filter((item) => !peerId || item.peerId === peerId),
    ),
  )
  const { startUpload, dismissUpload } = useFileUploads()

  if (uploads.length === 0) return null

  return (
    <ul className="uploads" aria-label="本地文件上传">
      {uploads.map((item) => (
        <li className="upload-row" key={item.id}>
          <span className="file-avatar">
            <File size={19} aria-hidden="true" />
          </span>
          <div className="transfer-content">
            <div className="file-name" title={item.file.name}>
              {item.file.name}
            </div>
            {item.status === 'uploading' ? (
              <FileProgress
                value={item.progress}
                label={`${item.file.name} 上传进度`}
                detail={
                  item.progress === 100
                    ? '正在保存到本地服务…'
                    : `正在上传到本地服务 · ${formatBytes(item.file.size)}`
                }
              />
            ) : (
              <p className="file-error" role="alert">
                {item.error}
              </p>
            )}
          </div>
          {item.status === 'failed' && (
            <button
              className="button button-small"
              type="button"
              disabled={!agentAvailable}
              onClick={() => void startUpload(item.peerId, item.file, item.id)}
            >
              重试上传
            </button>
          )}
          <button
            className="icon-button"
            type="button"
            title={item.status === 'failed' ? '移除' : '取消上传'}
            aria-label={
              item.status === 'failed'
                ? `移除 ${item.file.name}`
                : `取消上传 ${item.file.name}`
            }
            onClick={() => dismissUpload(item.id)}
          >
            <X size={17} aria-hidden="true" />
          </button>
        </li>
      ))}
    </ul>
  )
}

function FileProgress({
  value,
  label,
  detail,
}: {
  value: number
  label: string
  detail: string
}) {
  return (
    <div className="file-progress">
      <progress value={value} max={100} aria-label={label} />
      <span>{value}%</span>
      <small>{detail}</small>
    </div>
  )
}
