import { readFileSync } from 'node:fs'
import { runInNewContext } from 'node:vm'
import { expect, it } from 'vitest'

const script = readFileSync(
  new URL('../public/mobile/main.js', import.meta.url),
  'utf8',
)
const state = (body) => ({
  phase: 'approved',
  expires_at: Date.now() + 60_000,
  max_file_size: 1024,
  files: [],
  messages: [{ id: 'message-1', direction: 'to_mobile', body }],
})
const response = (body, status = 200) =>
  new Response(JSON.stringify(body), { status })
const drain = () => new Promise((resolve) => setImmediate(resolve))

class Element {
  hidden = true
  disabled = false
  value = ''
  textContent = ''
  files = []
  children = []
  listeners = new Map()
  setAttribute() {}
  append(...children) {
    this.children.push(...children)
  }
  replaceChildren() {
    this.children = []
  }
  addEventListener(event, listener) {
    this.listeners.set(event, listener)
  }
  async fire(event) {
    await this.listeners.get(event)?.({ preventDefault() {} })
  }
}

function page(fetch) {
  const elements = new Map()
  const timers = new Map()
  let counter = 0
  const element = (id) => {
    if (!elements.has(id)) elements.set(id, new Element())
    return elements.get(id)
  }
  runInNewContext(script, {
    document: { querySelector: element, createElement: () => new Element() },
    location: { hash: '', pathname: '/mobile/' },
    window: {
      setTimeout(callback, delay) {
        timers.set(++counter, { callback, delay })
        return counter
      },
      clearTimeout(id) {
        timers.delete(id)
      },
      setInterval() {},
      addEventListener() {},
    },
    fetch,
    AbortController,
    TextEncoder,
    crypto: globalThis.crypto,
  })
  return {
    element,
    poll() {
      const next = [...timers].find(([, timer]) => timer.delay === 1200)
      expect(next).toBeDefined()
      timers.delete(next[0])
      next[1].callback()
    },
    hasPoll: () => [...timers.values()].some((timer) => timer.delay === 1200),
  }
}

it('ignores an approved stale poll that resolves after the send ends the session', async () => {
  let release = () => {
    throw new Error('poll not started')
  }
  let reads = 0
  const p = page((path) => {
    if (path === '/mobile/messages')
      return Promise.resolve(response({ error: 'ended' }, 410))
    if (++reads === 1) return Promise.resolve(response(state('已显示文字')))
    return new Promise((resolve) => {
      release = resolve
    })
  })
  await drain()
  expect(p.element('#transfer').hidden).toBe(false)
  p.poll()
  p.element('#text-input').value = '发送测试'
  await p.element('#text-input').fire('input')
  await p.element('#text-form').fire('submit')
  expect(p.element('#transfer').hidden).toBe(true)
  expect(p.element('#texts').children).toHaveLength(0)
  release(response(state('不应重新显示')))
  await drain()
  expect(p.element('#transfer').hidden).toBe(true)
  expect(p.element('#texts').children).toHaveLength(0)
  expect(p.hasPoll()).toBe(false)
})

it('recovers the active session after a transient poll failure and on a fresh page', async () => {
  let reads = 0
  const fetch = () => {
    if (++reads === 2) return Promise.reject(new Error('offline'))
    return Promise.resolve(response(state('同一会话保留文字')))
  }
  const p = page(fetch)
  await drain()
  p.poll()
  await drain()
  expect(p.element('#transfer').hidden).toBe(true)
  p.poll()
  await drain()
  expect(p.element('#transfer').hidden).toBe(false)
  const reloaded = page(fetch)
  await drain()
  expect(
    reloaded.element('#texts').children[0].children[0].textContent,
  ).toContain('同一会话保留文字')
})
