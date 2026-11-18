import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, it } from 'vitest'
import { Chat } from './chat'
import type { Message, Peer } from './lanlink'

const peer: Peer = {
  id: 'peer-1',
  name: '另一台电脑',
  platform: 'macos',
  addresses: [],
  port: 38762,
  online: false,
  paired: true,
}

it.each([true, false])(
  'offers copying for received and sent messages with agentAvailable=%s',
  (agentAvailable) => {
    const messages: Message[] = (['incoming', 'outgoing'] as const).map(
      (direction, index) => ({
        id: `message-${index}`,
        peer_id: peer.id,
        direction,
        body: `第 ${index + 1} 条消息\n保留换行`,
        created_at: index,
        status: direction === 'outgoing' ? 'failed' : 'sent',
      }),
    )
    const html = renderToStaticMarkup(
      <QueryClientProvider client={new QueryClient()}>
        <Chat
          peer={peer}
          messages={messages}
          maxFileSize={undefined}
          agentAvailable={agentAvailable}
          isLoading={false}
          messagesError={null}
        />
      </QueryClientProvider>,
    )

    const copyButtons = html.match(/<button[^>]*aria-label="复制消息"[^>]*>/g)
    expect(copyButtons).toHaveLength(2)
    for (const button of copyButtons ?? []) {
      expect(button).toContain('type="button"')
      expect(button).not.toContain('disabled')
    }
    expect(html).toContain('重试')
    expect(html).toContain('第 1 条消息\n保留换行')
    expect(html).toContain('第 2 条消息\n保留换行')
  },
)
