import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, it } from 'vitest'
import type { MobileSession, MobileState, RememberedBrowser } from './lanlink'
import { MobileTransferDialog } from './mobile-transfer'

const browser: RememberedBrowser = {
  id: 'browser-1',
  name: 'iPhone Safari',
  created_at: 1_795_000_000_000,
  last_used_at: 1_795_100_000_000,
  expires_at: 1_802_800_000_000,
}

const session: MobileSession = {
  url: 'http://192.168.1.2:38762/mobile/#invite',
  phase: 'pending',
  code: null,
  expires_at: 1_900_000_000_000,
  max_file_size: 1024,
  files: [],
  remembered: false,
  remembered_browser_id: null,
}

function renderState(state: MobileState) {
  const client = new QueryClient()
  client.setQueryData(['agent', 'mobile'], state)
  return renderClient(client)
}

function renderClient(client: QueryClient) {
  return renderToStaticMarkup(
    <QueryClientProvider client={client}>
      <MobileTransferDialog onClose={() => {}} />
    </QueryClientProvider>,
  )
}

it('offers remembered approval by default and explains guest opt-out', () => {
  const html = renderState({ addresses: [], session })

  expect(html).toContain('type="checkbox" checked=""')
  expect(html).toContain('记住此浏览器，90 天内扫码免确认')
  expect(html).toContain('访客或公用设备请取消勾选，仅允许本次连接。')
})

it.each([null, { ...session, phase: 'approved' as const }])(
  'keeps remembered browsers and accessible forget actions available with session=%s',
  (activeSession) => {
    const html = renderState({
      addresses: ['192.168.1.2'],
      session: activeSession,
      remembered_browsers: [browser],
    })

    expect(html).toContain('aria-label="已记住的浏览器"')
    expect(html).toContain('aria-label="忘记 iPhone Safari"')
    expect(html).toContain('上次使用')
    expect(html).toContain(
      '忘记后下次扫码需重新确认；正在使用该浏览器的互传也会结束。',
    )
  },
)

it('accepts an older mobile snapshot without remembered browsers', () => {
  const html = renderState({ addresses: ['192.168.1.2'], session: null })

  expect(html).toContain('生成二维码')
  expect(html).toContain('暂无已记住的浏览器。')
})

it.each([
  [true, '已记住此浏览器，下次扫码可直接连接。'],
  [false, '本次为临时连接，下次扫码仍需电脑确认。'],
] as const)('shows remembered=%s status after approval', (remembered, copy) => {
  const html = renderState({
    addresses: [],
    session: { ...session, phase: 'approved', remembered },
  })

  expect(html).toContain(copy)
  expect(html).toContain('传输未加密，仅在可信 Wi-Fi 使用。')
})

it('renders plain text only for the active approved QR session', () => {
  const withText: MobileSession = {
    ...session,
    id: 'current-session',
    messages: [
      {
        id: 'msg-1',
        direction: 'to_computer',
        body: '<img src=x onerror=alert(1)>',
      },
    ],
  }
  expect(renderState({ addresses: [], session: withText })).not.toContain(
    '发送文字给手机',
  )
  const html = renderState({
    addresses: [],
    session: { ...withText, phase: 'approved' },
  })
  expect(html).toContain('发送文字给手机')
  expect(html).toContain('&lt;img src=x onerror=alert(1)&gt;')
  expect(html).not.toContain('<img src=x')
})

it('hides expired or failed polling text while retaining the file controls', async () => {
  const active: MobileSession = {
    ...session,
    id: 'current-session',
    phase: 'approved',
    messages: [
      { id: 'secret', direction: 'to_computer', body: '只在连接内显示' },
    ],
  }
  const expired = renderState({
    addresses: [],
    session: { ...active, expires_at: Date.now() - 1 },
  })
  expect(expired).not.toContain('只在连接内显示')
  expect(expired).toContain('发送文件')
  const client = new QueryClient()
  const queryKey = ['agent', 'mobile']
  client.setQueryData(queryKey, { addresses: [], session: active })
  await expect(
    client.fetchQuery({
      queryKey,
      queryFn: () => Promise.reject(new Error('离线')),
      retry: false,
    }),
  ).rejects.toThrow('离线')
  const failed = renderClient(client)
  expect(failed).not.toContain('只在连接内显示')
  expect(failed).toContain('发送文件')
})
