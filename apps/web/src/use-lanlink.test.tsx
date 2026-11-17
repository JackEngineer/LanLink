import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { renderToStaticMarkup } from 'react-dom/server'
import { afterEach, expect, it, vi } from 'vitest'
import { api } from './api'
import type { Transfer } from './lanlink'
import { useUi } from './ui-state'
import { agentKeys, useFileUploads } from './use-lanlink'

function uploadActions(client: QueryClient) {
  const capture = vi.fn<(actions: ReturnType<typeof useFileUploads>) => void>()
  function UploadCaller() {
    capture(useFileUploads())
    return null
  }
  renderToStaticMarkup(
    <QueryClientProvider client={client}>
      <UploadCaller />
    </QueryClientProvider>,
  )
  return capture.mock.calls[0][0]
}

function transfer(peerId: string, file: File): Transfer {
  return {
    id: file.name,
    peer_id: peerId,
    direction: 'outgoing',
    name: file.name,
    size: file.size,
    status: 'awaiting_accept',
    sha256: 'hash',
    offset: 0,
    error: null,
    created_at: 0,
  }
}

function pendingTransfer() {
  let resolve!: (value: Transfer) => void
  let reject!: (reason: Error) => void
  const promise = new Promise<Transfer>((onResolve, onReject) => {
    resolve = onResolve
    reject = onReject
  })
  return { promise, resolve, reject }
}

afterEach(() => {
  vi.restoreAllMocks()
  useUi.setState({ uploads: [] })
})

it('uploads one file at a time across chats and continues after a failure', async () => {
  const client = new QueryClient()
  const firstChat = uploadActions(client)
  const nextChat = uploadActions(client)
  const first = new File(['sample'], 'first.txt')
  const second = new File(['sample'], 'second.txt')
  const third = new File(['sample'], 'third.txt')
  const pendingFirst = pendingTransfer()
  const pendingSecond = pendingTransfer()
  const upload = vi
    .spyOn(api, 'upload')
    .mockReturnValueOnce(pendingFirst.promise)
    .mockReturnValueOnce(pendingSecond.promise)
    .mockResolvedValueOnce(transfer('other-peer', third))

  void firstChat.startUpload('peer', first)
  void firstChat.startUpload('peer', second, 'second')
  void nextChat.startUpload('other-peer', third)

  expect(
    upload.mock.calls.map(([peerId, file]) => [peerId, file.name]),
  ).toEqual([['peer', 'first.txt']])
  pendingFirst.resolve(transfer('peer', first))
  await vi.waitFor(() => expect(upload).toHaveBeenCalledTimes(2))
  expect(upload.mock.calls[1].slice(0, 2)).toEqual(['peer', second])

  pendingSecond.reject(new Error('文件保存失败'))
  await vi.waitFor(() =>
    expect(client.getQueryData(agentKeys.transfers)).toEqual([
      transfer('peer', first),
      transfer('other-peer', third),
    ]),
  )
  expect(upload.mock.calls[2].slice(0, 2)).toEqual(['other-peer', third])
  expect(useUi.getState().uploads).toMatchObject([
    { file: second, status: 'failed', error: '文件保存失败' },
  ])

  upload.mockResolvedValueOnce(transfer('peer', second))
  void nextChat.startUpload('peer', second, 'second')
  void firstChat.startUpload('peer', second, 'second')
  await vi.waitFor(() => expect(useUi.getState().uploads).toEqual([]))
  expect(upload).toHaveBeenCalledTimes(4)
  expect(upload.mock.calls[3].slice(0, 2)).toEqual(['peer', second])
})

it('skips a cancelled waiting file and continues after cancelling the active upload', async () => {
  const client = new QueryClient()
  const { startUpload, dismissUpload } = uploadActions(client)
  const active = new File(['sample'], 'active.txt')
  const waiting = new File(['sample'], 'waiting.txt')
  const next = new File(['sample'], 'next.txt')
  const upload = vi
    .spyOn(api, 'upload')
    .mockImplementationOnce(
      (_peerId, _file, _onProgress, signal) =>
        new Promise((_, reject) => {
          signal.addEventListener('abort', () => reject(signal.reason), {
            once: true,
          })
        }),
    )
    .mockResolvedValue(transfer('peer', next))

  void startUpload('peer', active, 'active')
  void startUpload('peer', waiting, 'waiting')
  void startUpload('peer', next, 'next')
  void startUpload('peer', next, 'next')
  dismissUpload('waiting')
  dismissUpload('active')

  await vi.waitFor(() =>
    expect(client.getQueryData(agentKeys.transfers)).toEqual([
      transfer('peer', next),
    ]),
  )
  expect(upload.mock.calls.map(([, file]) => file.name)).toEqual([
    'active.txt',
    'next.txt',
  ])
  expect(useUi.getState().uploads).toEqual([])
})
