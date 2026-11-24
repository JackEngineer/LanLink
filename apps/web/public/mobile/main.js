const status = document.querySelector('#status')
const timer = document.querySelector('#timer')
const confirmation = document.querySelector('#confirmation')
const remembered = document.querySelector('#remembered')
const code = document.querySelector('#code')
const transfer = document.querySelector('#transfer')
const limit = document.querySelector('#limit')
const files = document.querySelector('#files')
const empty = document.querySelector('#empty')
const upload = document.querySelector('#upload')
const input = document.querySelector('#file')
const submit = document.querySelector('#send')
const uploadProgress = document.querySelector('#upload-progress')
const progress = document.querySelector('#progress')
const percentage = document.querySelector('#percentage')
const sent = document.querySelector('#sent')
const uploadError = document.querySelector('#upload-error')
const connectionError = document.querySelector('#connection-error')
const retry = document.querySelector('#retry')

const numberFormat = new Intl.NumberFormat('zh-CN', {
  maximumFractionDigits: 1,
})
let invite = location.hash.slice(1)
let expiresAt = 0
let maxFileSize = 0
let inboxKey = ''
let uploading = false
let polling = false
let generation = 0
let clockOffset = 0
let connected = false
let activeUpload
let phase
const textPanel = document.querySelector('#text-panel')
const textForm = document.querySelector('#text-form')
const textInput = document.querySelector('#text-input')
const textSend = document.querySelector('#text-send')
const textError = document.querySelector('#text-error')
const texts = document.querySelector('#texts')
let textKey = ''
let sendingText = false
let pendingText

function textId() {
  // getRandomValues is available on the existing local HTTP phone page.
  const bytes = crypto.getRandomValues(new Uint8Array(16))
  bytes[6] = (bytes[6] & 15) | 64
  bytes[8] = (bytes[8] & 63) | 128
  const hex = Array.from(bytes, (byte) =>
    byte.toString(16).padStart(2, '0'),
  ).join('')
  return [
    hex.slice(0, 8),
    hex.slice(8, 12),
    hex.slice(12, 16),
    hex.slice(16, 20),
    hex.slice(20),
  ].join('-')
}

function showTexts(items) {
  textPanel.hidden = !Array.isArray(items)
  if (textPanel.hidden) return
  const key = JSON.stringify(items)
  if (key === textKey) return
  textKey = key
  texts.replaceChildren()
  for (const item of items) {
    const row = document.createElement('li')
    const line = document.createElement('span')
    line.textContent =
      (item.direction === 'to_computer' ? '手机：' : '电脑：') + item.body
    row.append(line)
    texts.append(row)
  }
}

function updateTextSend() {
  const body = textInput.value.trim()
  textSend.disabled =
    !connected ||
    phase !== 'approved' ||
    sendingText ||
    !body ||
    new TextEncoder().encode(body).length > 2048
}

textInput.addEventListener('input', updateTextSend)
textForm.addEventListener('submit', async (event) => {
  event.preventDefault()
  if (textSend.disabled) return
  const activeGeneration = generation
  const body = textInput.value.trim()
  if (pendingText?.body !== body) pendingText = { id: textId(), body }
  sendingText = true
  textInput.disabled = true
  updateTextSend()
  showError(textError, '')
  try {
    await request('messages', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(pendingText),
    })
    if (activeGeneration !== generation) return
    textInput.value = ''
    pendingText = undefined
  } catch (error) {
    if (activeGeneration !== generation) return
    if (terminal(error))
      endSession('互传已结束或已过期。请扫描电脑上的新二维码。')
    else showError(textError, error.message || '发送失败，请重试。')
  } finally {
    if (activeGeneration === generation) {
      sendingText = false
      textInput.disabled = false
      updateTextSend()
    }
  }
})

class RequestError extends Error {
  constructor(status, message) {
    super(message)
    this.status = status
  }
}

function formatBytes(bytes) {
  if (bytes < 1024) return numberFormat.format(bytes) + ' B'
  const units = ['KiB', 'MiB', 'GiB']
  const exponent = Math.min(Math.floor(Math.log2(bytes) / 10), units.length)
  return (
    numberFormat.format(bytes / 1024 ** exponent) + ' ' + units[exponent - 1]
  )
}

function setText(node, value) {
  if (node.textContent !== value) node.textContent = value
}

function showError(node, message) {
  if (!message) {
    node.hidden = true
    setText(node, '')
    return
  }
  node.hidden = false
  setText(node, message)
}

async function request(path, options) {
  const activeGeneration = generation
  const controller = new AbortController()
  const timeout = window.setTimeout(() => controller.abort(), 8_000)
  try {
    const response = await fetch('/mobile/' + path, {
      credentials: 'same-origin',
      cache: 'no-store',
      signal: controller.signal,
      ...options,
    })
    const data = await response.json().catch(() => null)
    if (!response.ok || !data) {
      throw new RequestError(
        response.status,
        data?.error || '电脑未返回互传状态。',
      )
    }
    const serverTime = Date.parse(response.headers.get('Date'))
    if (activeGeneration === generation && Number.isFinite(serverTime)) {
      clockOffset = serverTime - Date.now()
    }
    return data
  } finally {
    window.clearTimeout(timeout)
  }
}

function showTime() {
  if (!expiresAt) return
  const remaining = Math.ceil((expiresAt - Date.now() - clockOffset) / 1_000)
  if (remaining <= 0) {
    endSession('互传已过期。请扫描电脑上的新二维码。')
    return
  }
  const elapsedMinutes = Math.floor(remaining / 60)
  const seconds = String(remaining % 60).padStart(2, '0')
  setText(timer, '剩余 ' + elapsedMinutes + ':' + seconds)
}

function showInbox(items) {
  const inbox = items.filter((item) => item.direction === 'to_mobile')
  const key = JSON.stringify(inbox)
  if (key === inboxKey) return
  inboxKey = key

  files.replaceChildren()
  for (const item of inbox) {
    const row = document.createElement('li')
    const name = document.createElement('span')
    name.textContent = item.name
    const size = document.createElement('small')
    size.textContent = formatBytes(item.size)
    name.append(size)
    const link = document.createElement('a')
    link.textContent = '下载'
    link.setAttribute('aria-label', '下载 ' + item.name)
    link.href = '/mobile/files/' + encodeURIComponent(item.id)
    link.download = item.name
    row.append(name, link)
    files.append(row)
  }
  empty.hidden = inbox.length > 0
}

function showState(state) {
  connected = true
  phase = state.phase
  expiresAt = state.expires_at
  maxFileSize = state.max_file_size
  const waiting = phase === 'pending'
  const approved = phase === 'approved'

  setText(status, approved ? '已连接电脑' : '等待电脑确认')
  timer.hidden = false
  confirmation.hidden = !waiting
  setText(code, waiting ? state.code : '')
  transfer.hidden = !approved
  remembered.hidden = !approved
  setText(
    remembered,
    state.remembered
      ? '已记住此浏览器，90 天内再次扫码可直接连接。请使用同一浏览器；清除网站数据后需要重新确认。'
      : '本次为临时连接，下次扫码仍需电脑确认。',
  )
  showError(connectionError, '')
  showTime()

  if (phase === 'approved') {
    setText(limit, '单个文件最大 ' + formatBytes(maxFileSize))
    showInbox(state.files)
    showTexts(state.messages)
    updateTextSend()
    updateSend()
  }
}

function endSession(message) {
  generation++
  polling = false
  connected = false
  activeUpload?.abort()
  phase = undefined
  expiresAt = 0
  timer.hidden = true
  confirmation.hidden = true
  remembered.hidden = true
  code.textContent = ''
  texts.replaceChildren()
  textInput.value = ''
  textInput.disabled = false
  sendingText = false
  pendingText = undefined
  textKey = ''
  updateTextSend()
  transfer.hidden = true
  updateSend()
  showError(connectionError, '')
  setText(status, message)
}

function terminal(error) {
  return (
    error instanceof RequestError && [401, 403, 404, 410].includes(error.status)
  )
}

function schedulePoll(activeGeneration) {
  if (polling && activeGeneration === generation) {
    window.setTimeout(() => void poll(activeGeneration), 1_200)
  }
}

async function poll(activeGeneration) {
  if (!polling || activeGeneration !== generation) return
  try {
    const state = await request('state')
    if (!polling || activeGeneration !== generation) return
    showState(state)
  } catch (error) {
    if (!polling || activeGeneration !== generation) return
    if (terminal(error)) {
      endSession('互传已结束或已过期。请扫描电脑上的新二维码。')
    } else {
      connected = false
      transfer.hidden = true
      updateSend()
      updateTextSend()
      showError(connectionError, '暂时无法连接电脑，正在重试。')
    }
  }
  schedulePoll(activeGeneration)
}

function clearInvite() {
  if (!invite) return
  invite = ''
  history.replaceState(null, '', location.pathname)
}

async function connect() {
  const activeGeneration = ++generation
  polling = false
  retry.hidden = true
  showError(connectionError, '')
  setText(status, '正在连接电脑…')

  try {
    let state
    if (invite) {
      state = await request('claim', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ invite }),
      })
      if (activeGeneration !== generation) return
      clearInvite()
    } else {
      state = await request('state')
    }
    if (activeGeneration !== generation) return
    polling = true
    showState(state)
    schedulePoll(activeGeneration)
  } catch (error) {
    if (activeGeneration !== generation) return
    if (invite) {
      // A lost claim response may still have set the session cookie.
      try {
        const state = await request('state')
        if (activeGeneration !== generation) return
        clearInvite()
        polling = true
        showState(state)
        schedulePoll(activeGeneration)
        return
      } catch {}
      if (activeGeneration !== generation) return
      if (
        error instanceof RequestError &&
        error.status >= 400 &&
        error.status < 500
      ) {
        clearInvite()
        endSession('二维码已失效或无法使用。请扫描电脑上的新二维码。')
        return
      }
    }
    if (terminal(error)) {
      endSession('互传已结束或尚未连接。请扫描电脑上的二维码。')
      return
    }
    setText(status, '无法连接电脑')
    showError(connectionError, '请检查两台设备是否已连接同一个 Wi-Fi。')
    retry.hidden = false
  }
}

function updateSend() {
  const selected = input.files[0]
  const tooLarge = selected && selected.size > maxFileSize
  submit.disabled =
    uploading || !selected || tooLarge || phase !== 'approved' || !connected
  input.disabled = uploading
  if (tooLarge) {
    showError(uploadError, '文件大小超过 ' + formatBytes(maxFileSize) + '。')
  }
}

function sendFile(file) {
  return new Promise((resolve, reject) => {
    const xhr = new XMLHttpRequest()
    activeUpload = xhr
    xhr.open('POST', '/mobile/files')
    xhr.responseType = 'json'
    xhr.addEventListener(
      'loadend',
      () => {
        activeUpload = undefined
      },
      { once: true },
    )
    xhr.addEventListener('abort', () => reject(new Error('发送已中止。')))
    xhr.upload.addEventListener('progress', (event) => {
      if (!event.lengthComputable) return
      const value = Math.floor((100 * event.loaded) / event.total)
      progress.value = value
      percentage.textContent = value + '%'
    })
    xhr.addEventListener('load', () => {
      if (xhr.status >= 200 && xhr.status < 300 && xhr.response) {
        resolve(xhr.response)
      } else {
        reject(
          new RequestError(
            xhr.status,
            xhr.response?.error || '发送失败，请重试。',
          ),
        )
      }
    })
    xhr.addEventListener('error', () =>
      reject(new Error('发送中断，请检查网络后重试。')),
    )
    const data = new FormData()
    data.set('file', file)
    xhr.send(data)
  })
}

input.addEventListener('change', () => {
  showError(uploadError, '')
  sent.textContent = ''
  updateSend()
})

upload.addEventListener('submit', async (event) => {
  event.preventDefault()
  const file = input.files[0]
  if (submit.disabled || !file) return

  uploading = true
  submit.textContent = '正在发送…'
  showError(uploadError, '')
  sent.textContent = ''
  progress.removeAttribute('value')
  percentage.textContent = ''
  uploadProgress.hidden = false
  updateSend()
  try {
    const result = await sendFile(file)
    input.value = ''
    sent.textContent = '已发送 ' + result.name
  } catch (error) {
    if (phase === 'approved') showError(uploadError, error.message)
  } finally {
    uploading = false
    submit.textContent = '发送文件'
    uploadProgress.hidden = true
    updateSend()
  }
})

retry.addEventListener('click', () => void connect())
window.addEventListener('pagehide', () => {
  activeUpload?.abort()
})
window.setInterval(showTime, 500)
void connect()
