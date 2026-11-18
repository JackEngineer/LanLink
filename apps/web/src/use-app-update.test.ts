import { afterEach, expect, it, vi } from 'vitest'
import { waitForUpgrade } from './use-app-update'

afterEach(() => {
  vi.unstubAllGlobals()
  vi.useRealTimers()
})

it('waits through a disconnect and a stale Agent until the chosen version responds', async () => {
  vi.useFakeTimers()
  const version = '0.2.0-preview.3'
  const fetchMock = vi
    .fn<typeof fetch>()
    .mockRejectedValueOnce(new TypeError('connection closed'))
    .mockResolvedValueOnce(
      Response.json({ status: 'ok', version: '0.2.0-preview.2' }),
    )
    .mockResolvedValueOnce(Response.json({ status: 'ok', version }))
  vi.stubGlobal('fetch', fetchMock)

  const result = expect(waitForUpgrade(version)).resolves.toBeUndefined()
  await vi.runAllTimersAsync()
  await result
  expect(fetchMock).toHaveBeenCalledTimes(3)
})

it('shows rollback as soon as the old Agent reports the failure', async () => {
  vi.useFakeTimers()
  const version = '0.2.0-preview.3'
  const fetchMock = vi.fn<typeof fetch>().mockResolvedValueOnce(
    Response.json({
      status: 'ok',
      version: '0.2.0-preview.2',
      update_failed_version: version,
    }),
  )
  vi.stubGlobal('fetch', fetchMock)

  await expect(waitForUpgrade(version)).rejects.toThrow('已恢复到原版本')
  expect(fetchMock).toHaveBeenCalledTimes(1)
})

it('stops polling after the restart deadline', async () => {
  vi.useFakeTimers()
  vi.stubGlobal(
    'fetch',
    vi.fn<typeof fetch>().mockRejectedValue(new TypeError('connection closed')),
  )

  const result = expect(waitForUpgrade('0.2.0-preview.3')).rejects.toThrow(
    '无法确认升级结果',
  )
  await vi.runAllTimersAsync()
  await result
})
