import { useMutation, useQueryClient } from '@tanstack/react-query'
import { Link2Off, Menu, RefreshCw } from 'lucide-react'
import { useEffect, useState } from 'react'
import { api } from './api'
import { Chat, PairPeer, StartChat } from './chat'
import { DeviceIcon, InlineError, Modal } from './components'
import { PairingDialog, PairingRequests, Sidebar } from './devices'
import { platformName, type Peer } from './lanlink'
import { SettingsPage } from './settings'
import { Transfers } from './transfers'
import {
  agentKey,
  agentKeys,
  useAgentConnection,
  useAgentQuery,
} from './use-lanlink'
import { useUi } from './ui-state'

export function App() {
  const client = useQueryClient()
  const connection = useAgentConnection()
  const poll = connection === 'live' ? false : 2_500
  const health = useAgentQuery(agentKeys.health, api.health, 5_000)
  const device = useAgentQuery(agentKeys.device, api.device, poll)
  const devices = useAgentQuery(agentKeys.peers, api.peers, poll)
  const pairings = useAgentQuery(agentKeys.pairings, api.pairings, poll)
  const transfers = useAgentQuery(agentKeys.transfers, api.transfers, poll)
  const settings = useAgentQuery(agentKeys.settings, api.settings, poll)
  const view = useUi((state) => state.view)
  const peerId = useUi((state) => state.peerId)
  const sidebarOpen = useUi((state) => state.sidebarOpen)
  const pairingId = useUi((state) => state.pairingId)
  const setSidebarOpen = useUi((state) => state.setSidebarOpen)
  const setPairingId = useUi((state) => state.setPairingId)
  const peers = devices.data ?? []
  const peer = peers.find((item) => item.id === peerId)
  const messages = useAgentQuery(
    agentKeys.messages(peerId ?? ''),
    () => api.messages(peerId ?? ''),
    poll,
    view === 'chat' && Boolean(peer?.paired),
  )
  const [revokePeerId, setRevokePeerId] = useState<string | null>(null)
  const agentAvailable = health.isSuccess
  const hasPairings = Boolean(pairings.data?.length)
  const now = usePairingClock(hasPairings)
  const activePairings = (pairings.data ?? []).filter(
    (pairing) => pairing.expires_at > now,
  )
  const activeDialog = pairings.data?.find(
    (pairing) => pairing.id === pairingId,
  )
  const currentPairing = activePairings.find(
    (pairing) => pairing.peer_id === peerId,
  )
  const revoking = peers.find((item) => item.id === revokePeerId && item.paired)
  const incomingCount = (transfers.data ?? []).filter(
    (transfer) =>
      transfer.direction === 'incoming' && transfer.status === 'offered',
  ).length
  const agentStatus = !agentAvailable
    ? health.isPending
      ? '连接本地服务…'
      : '本地服务未连接'
    : connection === 'live'
      ? '本地服务已连接'
      : '实时连接重试中'
  const title =
    view === 'transfers'
      ? '文件传输'
      : view === 'settings'
        ? '设置'
        : (peer?.name ?? '聊天')

  return (
    <div
      className="app"
      onDragOver={(event) => {
        if (event.dataTransfer.types.includes('Files')) event.preventDefault()
      }}
      onDrop={(event) => {
        if (event.dataTransfer.types.includes('Files')) event.preventDefault()
      }}
    >
      <Sidebar
        device={device.data}
        peers={peers}
        agentStatus={agentStatus}
        agentAvailable={agentAvailable}
        peersLoading={devices.isPending}
        incomingCount={incomingCount}
      />
      <main className="main">
        <header className="main-header">
          <button
            className="icon-button mobile-only"
            type="button"
            aria-label="打开设备侧栏"
            aria-expanded={sidebarOpen}
            aria-controls="device-sidebar"
            onClick={() => setSidebarOpen(!sidebarOpen)}
          >
            <Menu size={22} aria-hidden="true" />
          </button>
          {view === 'chat' && peer && (
            <span className="device-avatar header-avatar">
              <DeviceIcon platform={peer.platform} size={21} />
            </span>
          )}
          <div className="header-heading">
            <h1 title={title}>{title}</h1>
            {view === 'chat' && peer && (
              <div className="header-status">
                <span
                  className={
                    agentAvailable && peer.online
                      ? 'status-dot status-online'
                      : 'status-dot'
                  }
                />
                {agentAvailable ? (peer.online ? '在线' : '离线') : '状态未知'}{' '}
                · {platformName(peer.platform)}
              </div>
            )}
          </div>
          {view === 'chat' && peer?.paired && (
            <button
              className="button button-small revoke-button"
              type="button"
              disabled={!agentAvailable}
              onClick={() => setRevokePeerId(peer.id)}
            >
              <Link2Off size={16} aria-hidden="true" /> 撤销配对
            </button>
          )}
        </header>
        {health.isError && (
          <div className="connection-banner" role="status">
            <span>无法连接 LANLink 本地服务。请检查 Agent 是否已启动。</span>
            <button
              className="button button-small"
              type="button"
              onClick={() =>
                void client.invalidateQueries({ queryKey: agentKey })
              }
            >
              <RefreshCw size={15} aria-hidden="true" /> 重试
            </button>
          </div>
        )}
        {!health.isError && devices.isError && (
          <div className="px-5 pt-3">
            <InlineError error={devices.error} />
          </div>
        )}
        {!health.isError && pairings.isError && (
          <div className="px-5 pt-3">
            <InlineError error={pairings.error} />
          </div>
        )}
        <PairingRequests pairings={activePairings} />
        {view === 'transfers' && (
          <Transfers
            records={transfers.data ?? []}
            peers={peers}
            agentAvailable={agentAvailable}
            isLoading={transfers.isPending}
            error={transfers.error}
          />
        )}
        {view === 'settings' && (
          <SettingsPage
            device={device.data}
            settings={settings.data}
            version={health.data?.version}
            loading={device.isPending || settings.isPending}
            agentAvailable={agentAvailable}
            error={device.error || settings.error}
          />
        )}
        {view === 'chat' &&
          (peer ? (
            peer.paired ? (
              <Chat
                key={peer.id}
                peer={peer}
                messages={messages.data ?? []}
                maxFileSize={settings.data?.max_file_size}
                agentAvailable={agentAvailable}
                isLoading={messages.isPending}
                messagesError={messages.error}
              />
            ) : (
              <PairPeer
                key={peer.id}
                peer={peer}
                pairing={currentPairing}
                agentAvailable={agentAvailable}
              />
            )
          ) : (
            <StartChat />
          ))}
      </main>
      {activeDialog && (
        <PairingDialog
          key={activeDialog.id}
          pairing={activeDialog}
          now={now}
          agentAvailable={agentAvailable}
          onClose={() => setPairingId(null)}
        />
      )}
      {revoking && (
        <RevokeDialog
          peer={revoking}
          agentAvailable={agentAvailable}
          onClose={() => setRevokePeerId(null)}
        />
      )}
    </div>
  )
}

function usePairingClock(active: boolean): number {
  const [now, setNow] = useState(Date.now)
  useEffect(() => {
    if (!active) return
    const timer = window.setInterval(() => setNow(Date.now()), 500)
    return () => window.clearInterval(timer)
  }, [active])
  return now
}

function RevokeDialog({
  peer,
  agentAvailable,
  onClose,
}: {
  peer: Peer
  agentAvailable: boolean
  onClose: () => void
}) {
  const client = useQueryClient()
  const revoke = useMutation({
    mutationFn: () => api.revoke(peer.id),
    onSuccess: () => {
      client.setQueryData<Peer[]>(agentKeys.peers, (items = []) =>
        items.map((item) =>
          item.id === peer.id ? { ...item, paired: false } : item,
        ),
      )
      onClose()
    },
    onSettled: () => void client.invalidateQueries({ queryKey: agentKey }),
  })

  return (
    <Modal title="撤销设备配对" onClose={onClose}>
      <p className="modal-copy">
        撤销与「{peer.name}」的配对将停止与此设备交换消息和文件。
      </p>
      <InlineError error={revoke.error} />
      <div className="modal-actions">
        <button className="button" type="button" onClick={onClose}>
          返回
        </button>
        <button
          className="button button-danger"
          type="button"
          disabled={revoke.isPending || !agentAvailable}
          onClick={() => revoke.mutate()}
        >
          撤销配对
        </button>
      </div>
    </Modal>
  )
}
