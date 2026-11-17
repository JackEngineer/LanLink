import type {
  Device,
  Health,
  Message,
  Pairing,
  Peer,
  Settings,
  Transfer,
  TransferAction,
} from './lanlink'

const base = '/api/v1'
const tokenHeader = 'X-LANLink-Token'

interface Ok {
  ok: true
}

function responseError(body: unknown, status: number): Error {
  if (
    body &&
    typeof body === 'object' &&
    'error' in body &&
    typeof body.error === 'string'
  ) {
    return new Error(body.error)
  }
  return new Error(`本地服务请求失败（HTTP ${status}）。`)
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  let response: Response
  try {
    response = await fetch(base + path, {
      credentials: 'same-origin',
      mode: 'same-origin',
      cache: 'no-store',
      ...init,
    })
  } catch {
    throw new Error('无法连接到 LANLink 本地服务。')
  }

  if (!response.ok) {
    const error: unknown = await response.json().catch(() => null)
    throw responseError(error, response.status)
  }
  return response.json()
}

async function sessionToken(): Promise<string> {
  const session = await request<unknown>('/session')
  if (
    !session ||
    typeof session !== 'object' ||
    !('token' in session) ||
    typeof session.token !== 'string' ||
    !session.token
  ) {
    throw new Error('本地服务未返回有效的会话。')
  }
  return session.token
}

async function mutate<T>(
  method: 'POST' | 'PATCH' | 'DELETE',
  path: string,
  body?: object,
) {
  const headers = new Headers({ [tokenHeader]: await sessionToken() })
  if (body) headers.set('Content-Type', 'application/json')
  return request<T>(path, {
    method,
    headers,
    body: body ? JSON.stringify(body) : undefined,
  })
}

const pathId = encodeURIComponent
const peerQuery = (id: string) =>
  new URLSearchParams({ peer_id: id }).toString()

async function upload(
  peerId: string,
  file: File,
  onProgress: (progress: number) => void,
  signal: AbortSignal,
): Promise<Transfer> {
  signal.throwIfAborted()
  const token = await sessionToken()
  signal.throwIfAborted()

  return new Promise((resolve, reject) => {
    const xhr = new XMLHttpRequest()
    xhr.open('POST', `${base}/transfers?${peerQuery(peerId)}`)
    xhr.responseType = 'json'
    xhr.setRequestHeader(tokenHeader, token)

    const abort = () => xhr.abort()
    signal.addEventListener('abort', abort, { once: true })
    xhr.addEventListener(
      'loadend',
      () => signal.removeEventListener('abort', abort),
      { once: true },
    )
    xhr.addEventListener('abort', () => reject(signal.reason))
    xhr.addEventListener('error', () =>
      reject(new Error('上传中断，请检查本地服务后重试。')),
    )
    xhr.upload.addEventListener('progress', (event) => {
      if (event.lengthComputable)
        onProgress(Math.floor((event.loaded / event.total) * 100))
    })
    xhr.addEventListener('load', () => {
      if (xhr.status >= 200 && xhr.status < 300 && xhr.response) {
        resolve(xhr.response)
      } else {
        reject(responseError(xhr.response, xhr.status))
      }
    })

    const data = new FormData()
    data.set('file', file)
    xhr.send(data)
  })
}

export const api = {
  health: () => request<Health>('/health'),
  device: () => request<Device>('/device'),
  rename: (name: string) => mutate<Device>('PATCH', '/device', { name }),
  peers: () => request<Peer[]>('/devices'),
  pair: (id: string) =>
    mutate<{ id: string }>('POST', `/devices/${pathId(id)}/pair`),
  revoke: (id: string) => mutate<Ok>('DELETE', `/devices/${pathId(id)}/pair`),
  pairings: () => request<Pairing[]>('/pairings'),
  confirm: (id: string, accept: boolean) =>
    mutate<Ok>('POST', `/pairings/${pathId(id)}/confirm`, { accept }),
  messages: (peerId: string) =>
    request<Message[]>(`/messages?${peerQuery(peerId)}`),
  send: (peerId: string, body: string) =>
    mutate<Message>('POST', '/messages', { peer_id: peerId, body }),
  retryMessage: (id: string) =>
    mutate<Ok>('POST', `/messages/${pathId(id)}/retry`),
  transfers: () => request<Transfer[]>('/transfers'),
  upload,
  transferAction: (id: string, action: TransferAction) =>
    mutate<Ok>('POST', `/transfers/${pathId(id)}/${action}`),
  downloadUrl: (id: string) => `${base}/transfers/${pathId(id)}/download`,
  settings: () => request<Settings>('/settings'),
  saveSettings: (maxFileSize: number) =>
    mutate<Settings>('PATCH', '/settings', { max_file_size: maxFileSize }),
  socket: async (signal: AbortSignal) => {
    const token = await sessionToken()
    signal.throwIfAborted()
    const url = new URL(base + '/ws', window.location.href)
    url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:'
    return new WebSocket(url, ['lanlink', token])
  },
}
