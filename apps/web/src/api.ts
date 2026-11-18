import type {
  Device,
  Health,
  Message,
  MobileFile,
  MobileState,
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
      referrerPolicy: 'same-origin',
      cache: 'no-store',
      ...init,
    })
  } catch {
    init?.signal?.throwIfAborted()
    throw new Error('无法连接到 LANLink 本地服务。')
  }

  if (!response.ok) {
    const error: unknown = await response.json().catch(() => null)
    throw responseError(error, response.status)
  }
  return response.json()
}

async function sessionToken(signal?: AbortSignal): Promise<string> {
  const session = await request<unknown>('/session', { signal })
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

class UploadBusyError extends Error {}

const busyRetryDelays = [50, 150, 400, 900]

function waitForUploadRetry(delay: number, signal: AbortSignal): Promise<void> {
  signal.throwIfAborted()
  return new Promise((resolve) => {
    const finish = () => {
      clearTimeout(timer)
      signal.removeEventListener('abort', finish)
      resolve()
    }
    const timer = setTimeout(finish, delay)
    signal.addEventListener('abort', finish, { once: true })
  })
}

async function upload(
  peerId: string,
  file: File,
  onProgress: (progress: number) => void,
  signal: AbortSignal,
): Promise<Transfer> {
  signal.throwIfAborted()
  const token = await sessionToken(signal)
  for (let attempt = 0; ; attempt += 1) {
    try {
      return await uploadFile<Transfer>(
        `/transfers?${peerQuery(peerId)}`,
        file,
        token,
        onProgress,
        signal,
      )
    } catch (error) {
      const delay = busyRetryDelays[attempt]
      if (!(error instanceof UploadBusyError) || delay === undefined)
        throw error
      onProgress(0)
      await waitForUploadRetry(delay, signal)
    }
  }
}

function uploadFile<T>(
  path: string,
  file: File,
  token: string,
  onProgress: (progress: number) => void,
  signal: AbortSignal,
): Promise<T> {
  signal.throwIfAborted()

  return new Promise((resolve, reject) => {
    const xhr = new XMLHttpRequest()
    xhr.open('POST', base + path)
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
        const error = responseError(xhr.response, xhr.status)
        if (
          xhr.status === 400 &&
          error.message === 'another upload is in progress'
        )
          reject(new UploadBusyError(error.message))
        else reject(error)
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
  mobile: async (signal?: AbortSignal) =>
    request<MobileState>('/mobile', {
      headers: { [tokenHeader]: await sessionToken(signal) },
      signal,
    }),
  startMobile: (address: string) =>
    mutate<MobileState>('POST', '/mobile', { address }),
  approveMobile: (code: string, remember = true) =>
    mutate<MobileState>('POST', '/mobile/approve', { code, remember }),
  forgetMobileBrowser: (id: string) =>
    mutate<MobileState>('DELETE', `/mobile/browsers/${pathId(id)}`),
  endMobile: () => mutate<MobileState>('DELETE', '/mobile'),
  uploadMobile: async (
    file: File,
    onProgress: (progress: number) => void,
    signal: AbortSignal,
  ) =>
    uploadFile<MobileFile>(
      '/mobile/files',
      file,
      await sessionToken(signal),
      onProgress,
      signal,
    ),
  mobileDownloadUrl: (id: string) => `${base}/mobile/files/${pathId(id)}`,
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
