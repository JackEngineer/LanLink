import { useMutation, useQueryClient } from '@tanstack/react-query'
import {
  ArrowLeftRight,
  Check,
  ChevronRight,
  Link2,
  MessageSquare,
  Radio,
  Settings2,
  X,
} from 'lucide-react'
import { api } from './api'
import { DeviceIcon, InlineError, Modal } from './components'
import { platformName, type Device, type Pairing, type Peer } from './lanlink'
import { agentKey } from './use-lanlink'
import { useUi } from './ui-state'

interface SidebarProps {
  device: Device | undefined
  peers: Peer[]
  agentStatus: string
  agentAvailable: boolean
  peersLoading: boolean
  incomingCount: number
}

export function Sidebar({
  device,
  peers,
  agentStatus,
  agentAvailable,
  peersLoading,
  incomingCount,
}: SidebarProps) {
  const peerId = useUi((state) => state.peerId)
  const view = useUi((state) => state.view)
  const open = useUi((state) => state.sidebarOpen)
  const setView = useUi((state) => state.setView)
  const selectPeer = useUi((state) => state.selectPeer)
  const setSidebarOpen = useUi((state) => state.setSidebarOpen)
  const paired = peers.filter((peer) => peer.paired)
  const nearby = peers.filter((peer) => !peer.paired && peer.online)

  return (
    <>
      {open && (
        <button
          type="button"
          className="sidebar-scrim"
          aria-label="关闭设备侧栏"
          onClick={() => setSidebarOpen(false)}
        />
      )}
      <aside
        id="device-sidebar"
        className={open ? 'sidebar sidebar-open' : 'sidebar'}
        aria-label="设备与导航"
      >
        <div className="brand">
          <img className="brand-mark" src="/logo.png" alt="" />
          <span>LANLink</span>
          <button
            type="button"
            className="icon-button mobile-only ml-auto"
            aria-label="关闭设备侧栏"
            onClick={() => setSidebarOpen(false)}
          >
            <X size={20} aria-hidden="true" />
          </button>
        </div>
        <nav className="main-nav" aria-label="主要导航">
          <button
            type="button"
            aria-current={view === 'chat' ? 'page' : undefined}
            onClick={() => setView('chat')}
          >
            <MessageSquare size={17} aria-hidden="true" />
            聊天
          </button>
          <button
            type="button"
            aria-current={view === 'transfers' ? 'page' : undefined}
            onClick={() => setView('transfers')}
          >
            <ArrowLeftRight size={17} aria-hidden="true" />
            文件传输
            {incomingCount > 0 && (
              <span
                className="nav-count"
                aria-label={`${incomingCount} 个文件待接收`}
              >
                {incomingCount}
              </span>
            )}
          </button>
          <button
            type="button"
            aria-current={view === 'settings' ? 'page' : undefined}
            onClick={() => setView('settings')}
          >
            <Settings2 size={17} aria-hidden="true" />
            设置
          </button>
        </nav>
        <div className="device-list">
          <DeviceGroup
            title="已配对"
            peers={paired}
            agentAvailable={agentAvailable}
            selectedId={view === 'chat' ? peerId : null}
            onSelect={selectPeer}
            empty={peersLoading ? '正在读取设备…' : '暂无已配对设备'}
          />
          <DeviceGroup
            title="附近设备"
            peers={nearby}
            agentAvailable={agentAvailable}
            selectedId={view === 'chat' ? peerId : null}
            onSelect={selectPeer}
            empty={peersLoading ? '正在读取设备…' : '暂未发现其他设备'}
          />
        </div>
        <div className="sidebar-footer">
          <span className="device-avatar">
            <DeviceIcon platform={device?.platform} />
          </span>
          <div className="min-w-0">
            <div className="local-name" title={device?.name}>
              {device?.name || '此设备'}
            </div>
            <div className="service-status">
              <span
                className={
                  agentAvailable ? 'status-dot status-online' : 'status-dot'
                }
              />
              {agentStatus}
            </div>
          </div>
        </div>
      </aside>
    </>
  )
}

function DeviceGroup({
  title,
  peers,
  agentAvailable,
  selectedId,
  onSelect,
  empty,
}: {
  title: string
  peers: Peer[]
  agentAvailable: boolean
  selectedId: string | null
  onSelect: (id: string) => void
  empty: string
}) {
  return (
    <div className="device-group">
      <h2>{title}</h2>
      {peers.length === 0 ? (
        <p className="device-empty">{empty}</p>
      ) : (
        <ul>
          {peers.map((peer) => (
            <li key={peer.id}>
              <button
                type="button"
                className="device-row"
                aria-pressed={selectedId === peer.id}
                onClick={() => onSelect(peer.id)}
              >
                <span className="device-avatar">
                  <DeviceIcon platform={peer.platform} />
                </span>
                <span className="min-w-0 flex-1">
                  <span className="device-name" title={peer.name}>
                    {peer.name}
                  </span>
                  <span className="device-detail">
                    {platformName(peer.platform)} ·{' '}
                    {agentAvailable
                      ? peer.online
                        ? '在线'
                        : '离线'
                      : '状态未知'}
                  </span>
                </span>
                <span
                  className={
                    agentAvailable && peer.online
                      ? 'status-dot status-online'
                      : 'status-dot'
                  }
                  aria-hidden="true"
                />
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  )
}

export function PairingRequests({ pairings }: { pairings: Pairing[] }) {
  const showPairing = useUi((state) => state.setPairingId)
  if (pairings.length === 0) return null

  return (
    <div className="request-bar" aria-label="设备配对">
      {pairings.map((pairing) => (
        <button
          type="button"
          key={pairing.id}
          onClick={() => showPairing(pairing.id)}
        >
          <Link2 size={17} aria-hidden="true" />
          <span className="min-w-0 truncate">
            {pairing.local_confirmed ? '等待配对确认' : '有配对请求'} ·{' '}
            {pairing.peer_name}
          </span>
          <ChevronRight size={16} aria-hidden="true" />
        </button>
      ))}
    </div>
  )
}

export function PairingDialog({
  pairing,
  now,
  agentAvailable,
  onClose,
}: {
  pairing: Pairing
  now: number
  agentAvailable: boolean
  onClose: () => void
}) {
  const client = useQueryClient()
  const expired = pairing.expires_at <= now
  const seconds = Math.max(0, Math.ceil((pairing.expires_at - now) / 1000))
  const confirm = useMutation({
    mutationFn: (accept: boolean) => api.confirm(pairing.id, accept),
    onSettled: () => void client.invalidateQueries({ queryKey: agentKey }),
    onSuccess: (_result, accept) => {
      if (!accept) onClose()
    },
  })
  const localConfirmed =
    pairing.local_confirmed || (confirm.isSuccess && confirm.variables)

  return (
    <Modal title="确认设备配对" onClose={onClose}>
      <p className="modal-copy">
        请在「{pairing.peer_name}」上核对安全码。两台设备都需要确认。
      </p>
      <div className="security-code" aria-label={`安全码 ${pairing.code}`}>
        {pairing.code}
      </div>
      <div className="pairing-checks">
        <span>
          {localConfirmed ? <Check size={16} /> : <Radio size={16} />} 此设备
          {localConfirmed ? '已确认' : '待确认'}
        </span>
        <span>
          {pairing.remote_confirmed ? <Check size={16} /> : <Radio size={16} />}{' '}
          对方{pairing.remote_confirmed ? '已确认' : '待确认'}
        </span>
      </div>
      <p className="modal-detail">
        {expired ? '此配对请求已过期。' : `${seconds} 秒后过期`}
      </p>
      <InlineError error={confirm.error} />
      <div className="modal-actions">
        {expired ? (
          <button className="button" type="button" onClick={onClose}>
            关闭
          </button>
        ) : (
          <>
            <button
              className="button"
              type="button"
              disabled={confirm.isPending || !agentAvailable}
              onClick={() => confirm.mutate(false)}
            >
              拒绝
            </button>
            <button
              className="button button-primary"
              type="button"
              disabled={
                confirm.isPending || Boolean(localConfirmed) || !agentAvailable
              }
              onClick={() => confirm.mutate(true)}
            >
              {localConfirmed ? '已确认' : '安全码一致，确认'}
            </button>
          </>
        )}
      </div>
    </Modal>
  )
}
