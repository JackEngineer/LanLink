import { afterEach, describe, expect, it, vi } from 'vitest'
import { api } from './api'
import type { Transfer } from './lanlink'

afterEach(() => {
  vi.unstubAllGlobals()
})

function mockFetch(...responses: Response[]) {
  const fetchMock = vi.fn<typeof fetch>()
  for (const response of responses) fetchMock.mockResolvedValueOnce(response)
  vi.stubGlobal('fetch', fetchMock)
  return fetchMock
}

describe('local Agent requests', () => {
  it('bootstraps a same-origin session before a mutation and authenticates the encoded request', async () => {
    const fetchMock = mockFetch(
      Response.json({ token: 'launch-token' }),
      Response.json({ ok: true }),
    )

    await expect(api.confirm('pair/name?', false)).resolves.toEqual({
      ok: true,
    })

    expect(fetchMock).toHaveBeenNthCalledWith(
      1,
      '/api/v1/session',
      expect.objectContaining({
        mode: 'same-origin',
        credentials: 'same-origin',
        cache: 'no-store',
      }),
    )
    expect(fetchMock).toHaveBeenNthCalledWith(
      2,
      '/api/v1/pairings/pair%2Fname%3F/confirm',
      expect.objectContaining({
        method: 'POST',
        headers: new Headers({
          'X-LANLink-Token': 'launch-token',
          'Content-Type': 'application/json',
        }),
        body: JSON.stringify({ accept: false }),
      }),
    )
  })

  it('keeps reads on loopback and surfaces the Agent error message', async () => {
    const fetchMock = mockFetch(
      Response.json({ error: '此设备未配对。' }, { status: 403 }),
    )

    await expect(api.messages('device / #1')).rejects.toThrow('此设备未配对。')
    expect(fetchMock).toHaveBeenCalledExactlyOnceWith(
      '/api/v1/messages?peer_id=device+%2F+%231',
      expect.objectContaining({ credentials: 'same-origin' }),
    )
  })

  it('does not send a mutation when the Agent returns no session token', async () => {
    const fetchMock = mockFetch(Response.json(null))

    await expect(api.send('peer', 'message')).rejects.toThrow('有效的会话')
    expect(fetchMock).toHaveBeenCalledTimes(1)
  })

  it('uses a same-origin WebSocket with the per-launch token as a subprotocol', async () => {
    mockFetch(Response.json({ token: 'launch-token' }))
    const connect = vi.fn()
    vi.stubGlobal('window', { location: { href: 'http://127.0.0.1:38761/' } })
    vi.stubGlobal(
      'WebSocket',
      class {
        constructor(url: URL, protocols: string[]) {
          connect(url.href, protocols)
        }
      },
    )

    await api.socket(new AbortController().signal)

    expect(connect).toHaveBeenCalledExactlyOnceWith(
      'ws://127.0.0.1:38761/api/v1/ws',
      ['lanlink', 'launch-token'],
    )
  })

  it('submits the original browser File in multipart and reports progress', async () => {
    mockFetch(Response.json({ token: 'launch-token' }))
    const file = new File(['sample'], 'sample.txt')
    const transfer: Transfer = {
      id: 'transfer-1',
      peer_id: 'device+two',
      direction: 'outgoing',
      name: file.name,
      size: file.size,
      status: 'offered',
      sha256: 'hash',
      offset: 0,
      error: null,
      created_at: 0,
    }
    const progress = vi.fn()
    const open = vi.fn()
    const header = vi.fn()
    const sentFile = vi.fn()

    vi.stubGlobal(
      'XMLHttpRequest',
      class extends EventTarget {
        upload = new EventTarget()
        response = transfer
        status = 201
        open = open
        setRequestHeader = header

        send(body: FormData) {
          sentFile(body.get('file'))
          this.upload.dispatchEvent(
            Object.assign(new Event('progress'), {
              lengthComputable: true,
              loaded: 1,
              total: 2,
            }),
          )
          this.dispatchEvent(new Event('load'))
          this.dispatchEvent(new Event('loadend'))
        }
      },
    )

    await expect(
      api.upload(
        transfer.peer_id,
        file,
        progress,
        new AbortController().signal,
      ),
    ).resolves.toEqual(transfer)

    expect(open).toHaveBeenCalledExactlyOnceWith(
      'POST',
      '/api/v1/transfers?peer_id=device%2Btwo',
    )
    expect(header).toHaveBeenCalledExactlyOnceWith(
      'X-LANLink-Token',
      'launch-token',
    )
    expect(sentFile.mock.calls[0][0]).toBe(file)
    expect(progress).toHaveBeenCalledExactlyOnceWith(50)
  })
})
