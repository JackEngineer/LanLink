import { afterEach, describe, expect, it, vi } from 'vitest'
import { api } from './api'
import type { Transfer } from './lanlink'

afterEach(() => {
  vi.unstubAllGlobals()
  vi.useRealTimers()
})

function mockFetch(...responses: Response[]) {
  const fetchMock = vi.fn<typeof fetch>()
  for (const response of responses) fetchMock.mockResolvedValueOnce(response)
  vi.stubGlobal('fetch', fetchMock)
  return fetchMock
}

function mockUploadResponses(
  ...responses: { status: number; body: unknown }[]
) {
  const send = vi.fn()
  vi.stubGlobal(
    'XMLHttpRequest',
    class extends EventTarget {
      upload = new EventTarget()
      open = vi.fn()
      setRequestHeader = vi.fn()
      response: unknown
      status = 0

      send(data: FormData) {
        send(data.get('file'))
        const response = responses.shift()
        this.status = response?.status ?? 500
        this.response = response?.body
        this.dispatchEvent(new Event('load'))
        this.dispatchEvent(new Event('loadend'))
      }
    },
  )
  return send
}

const busyUpload = {
  status: 400,
  body: { error: 'another upload is in progress' },
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

  it('retries the same File after the Agent refuses admission before reading it', async () => {
    vi.useFakeTimers()
    mockFetch(Response.json({ token: 'launch-token' }))
    const file = new File(['sample'], 'sample.txt')
    const transfer = { id: 'transfer-1', name: file.name }
    const send = mockUploadResponses(busyUpload, {
      status: 200,
      body: transfer,
    })
    const result = api
      .upload('peer', file, vi.fn(), new AbortController().signal)
      .catch((error: unknown) => error)

    await vi.runAllTimersAsync()
    await expect(result).resolves.toEqual(transfer)
    expect(send.mock.calls).toEqual([[file], [file]])
  })

  it('stops retrying when the Agent stays busy', async () => {
    vi.useFakeTimers()
    mockFetch(Response.json({ token: 'launch-token' }))
    const send = mockUploadResponses(...Array(6).fill(busyUpload))
    const result = expect(
      api.upload(
        'peer',
        new File([], 'sample.txt'),
        vi.fn(),
        new AbortController().signal,
      ),
    ).rejects.toThrow('another upload is in progress')

    await vi.runAllTimersAsync()
    await result
    expect(send).toHaveBeenCalledTimes(5)
  })

  it('sends no second request when cancelled during an admission retry delay', async () => {
    vi.useFakeTimers()
    mockFetch(Response.json({ token: 'launch-token' }))
    const send = mockUploadResponses(busyUpload)
    const controller = new AbortController()
    const cancelled = new Error('cancelled')
    const result = api
      .upload('peer', new File([], 'sample.txt'), vi.fn(), controller.signal)
      .catch((error: unknown) => error)

    await vi.advanceTimersByTimeAsync(0)
    expect(send).toHaveBeenCalledTimes(1)
    controller.abort(cancelled)
    await expect(result).resolves.toBe(cancelled)
    expect(vi.getTimerCount()).toBe(0)
    expect(send).toHaveBeenCalledTimes(1)
  })

  it.each([
    { status: 400, body: { error: 'file exceeds configured limit' } },
    { status: 500, body: busyUpload.body },
  ])(
    'does not retry an unrelated failure with status $status',
    async (response) => {
      vi.useFakeTimers()
      mockFetch(Response.json({ token: 'launch-token' }))
      const send = mockUploadResponses(response)
      await expect(
        api.upload(
          'peer',
          new File([], 'sample.txt'),
          vi.fn(),
          new AbortController().signal,
        ),
      ).rejects.toThrow(response.body.error)
      expect(send).toHaveBeenCalledTimes(1)
    },
  )
})
