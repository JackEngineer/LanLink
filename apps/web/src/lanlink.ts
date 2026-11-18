export interface Health {
  status: 'ok'
  version: string
}

export type Update =
  | { status: 'current' }
  | { status: 'available'; version: string; download_url: string }
  | { status: 'unsupported' }

export interface Device {
  id: string
  name: string
  platform: string
}

export interface Peer extends Device {
  addresses: string[]
  port: number
  online: boolean
  paired: boolean
}

export interface Pairing {
  id: string
  peer_id: string
  peer_name: string
  code: string
  local_confirmed: boolean
  remote_confirmed: boolean
  expires_at: number
}

export type Direction = 'incoming' | 'outgoing'

export interface Message {
  id: string
  peer_id: string
  direction: Direction
  body: string
  created_at: number
  status: 'sending' | 'sent' | 'pending' | 'failed'
}

export type TransferStatus =
  | 'offered'
  | 'awaiting_accept'
  | 'transferring'
  | 'paused'
  | 'completed'
  | 'rejected'
  | 'cancelled'
  | 'failed'

export type TransferAction = 'accept' | 'reject' | 'cancel' | 'retry'

export interface Transfer {
  id: string
  peer_id: string
  direction: Direction
  name: string
  size: number
  sha256: string
  offset: number
  status: TransferStatus
  failure_kind?: 'source_unavailable'
  error: string | null
  created_at: number
}

export interface MobileFile {
  id: string
  name: string
  size: number
  direction: 'to_computer' | 'to_mobile'
}

export interface MobileSession {
  url: string
  phase: 'inviting' | 'pending' | 'approved'
  code: string | null
  expires_at: number
  max_file_size: number
  files: MobileFile[]
  remembered: boolean
  remembered_browser_id: string | null
}

export interface RememberedBrowser {
  id: string
  name: string
  created_at: number
  last_used_at: number
  expires_at: number
}

export interface MobileState {
  addresses: string[]
  session: MobileSession | null
  remembered_browsers?: RememberedBrowser[]
}

export interface Settings {
  download_dir: string
  max_file_size: number
}

export const MEBIBYTE = 1024 ** 2
export const MAX_FILE_BYTES = 2 * 1024 ** 3
export const MESSAGE_MAX_BYTES = 16 * 1024

export const transferLabels: Record<TransferStatus, string> = {
  offered: '等待接收',
  awaiting_accept: '等待接收',
  transferring: '传输中',
  paused: '已暂停',
  completed: '已完成',
  rejected: '已拒绝',
  cancelled: '已取消',
  failed: '传输失败',
}

export const messageLabels: Record<Message['status'], string> = {
  sent: '已送达',
  sending: '发送中',
  pending: '待发送',
  failed: '发送失败',
}

export function transferActions(
  transfer: Pick<Transfer, 'status' | 'direction' | 'failure_kind'>,
): TransferAction[] {
  const { status, direction } = transfer
  if (status === 'offered' && direction === 'incoming')
    return ['accept', 'reject']
  if (
    status === 'offered' ||
    status === 'awaiting_accept' ||
    status === 'transferring'
  )
    return ['cancel']
  if (status === 'paused')
    return direction === 'outgoing' ? ['retry', 'cancel'] : ['cancel']
  if (status === 'failed' && direction === 'outgoing')
    return transfer.failure_kind === 'source_unavailable'
      ? ['cancel']
      : ['retry', 'cancel']
  return []
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return new Intl.NumberFormat('zh-CN').format(bytes) + ' B'
  const units = ['KiB', 'MiB', 'GiB', 'TiB']
  const exponent = Math.min(Math.floor(Math.log2(bytes) / 10), units.length)
  return (
    new Intl.NumberFormat('zh-CN', { maximumFractionDigits: 1 }).format(
      bytes / 1024 ** exponent,
    ) +
    ' ' +
    units[exponent - 1]
  )
}

export function percent(offset: number, size: number): number {
  return size > 0
    ? Math.min(100, Math.max(0, Math.floor((offset / size) * 100)))
    : 0
}

const timeFormatter = new Intl.DateTimeFormat('zh-CN', {
  hour: '2-digit',
  minute: '2-digit',
})
const dateFormatter = new Intl.DateTimeFormat('zh-CN', {
  month: 'short',
  day: 'numeric',
  hour: '2-digit',
  minute: '2-digit',
})

export function formatTime(timestamp: number): string {
  const date = new Date(timestamp)
  if (!Number.isFinite(date.getTime())) return '时间未知'
  return date.toDateString() === new Date().toDateString()
    ? timeFormatter.format(date)
    : dateFormatter.format(date)
}

export function isoTime(timestamp: number): string | undefined {
  const date = new Date(timestamp)
  return Number.isFinite(date.getTime()) ? date.toISOString() : undefined
}

export function platformName(platform: string): string {
  if (platform.toLowerCase() === 'macos') return 'macOS'
  if (platform.toLowerCase() === 'windows') return 'Windows'
  if (platform.toLowerCase() === 'linux') return 'Linux'
  return platform
}

export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : '操作失败，请重试。'
}
