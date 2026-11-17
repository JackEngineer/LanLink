import { describe, expect, it } from 'vitest'
import {
  formatBytes,
  formatTime,
  isoTime,
  percent,
  transferActions,
  type Direction,
  type TransferStatus,
} from './lanlink'

it('keeps historical records readable when a peer supplied an invalid date', () => {
  for (const timestamp of [Number.MAX_SAFE_INTEGER, NaN, Infinity]) {
    expect(formatTime(timestamp)).toBe('时间未知')
    expect(isoTime(timestamp)).toBeUndefined()
  }
  expect(isoTime(0)).toBe('1970-01-01T00:00:00.000Z')
})

describe('transfer actions', () => {
  it('requires the receiver to accept or reject a new incoming offer', () => {
    expect(
      transferActions({ direction: 'incoming', status: 'offered' }),
    ).toEqual(['accept', 'reject'])
    expect(
      transferActions({ direction: 'outgoing', status: 'awaiting_accept' }),
    ).toEqual(['cancel'])
  })

  it('allows retry and cancel for paused or retryable failed outgoing transfers', () => {
    expect(
      transferActions({ direction: 'outgoing', status: 'paused' }),
    ).toEqual(['retry', 'cancel'])
    expect(
      transferActions({ direction: 'outgoing', status: 'failed' }),
    ).toEqual(['retry', 'cancel'])
  })

  it('allows only cancel for an outgoing transfer whose source is unavailable', () => {
    expect(
      transferActions({
        direction: 'outgoing',
        status: 'failed',
        failure_kind: 'source_unavailable',
      }),
    ).toEqual(['cancel'])
  })

  it('only lets the receiver cancel a paused transfer', () => {
    expect(
      transferActions({ direction: 'incoming', status: 'paused' }),
    ).toEqual(['cancel'])
    expect(
      transferActions({ direction: 'incoming', status: 'failed' }),
    ).toEqual([])
  })

  it.each<TransferStatus>(['completed', 'rejected', 'cancelled'])(
    'leaves %s transfers final',
    (status) => {
      const directions: Direction[] = ['incoming', 'outgoing']
      for (const direction of directions) {
        expect(transferActions({ direction, status })).toEqual([])
      }
    },
  )
})

describe('file size and progress', () => {
  it('displays binary units at their exact boundaries', () => {
    expect(formatBytes(1023)).toBe('1,023 B')
    expect(formatBytes(1024)).toBe('1 KiB')
    expect(formatBytes(1.5 * 1024 ** 2)).toBe('1.5 MiB')
    expect(formatBytes(2 * 1024 ** 3)).toBe('2 GiB')
  })

  it('never reports an invalid percent for empty or offset-mismatched snapshots', () => {
    expect(percent(0, 0)).toBe(0)
    expect(percent(-1, 100)).toBe(0)
    expect(percent(2, 3)).toBe(66)
    expect(percent(110, 100)).toBe(100)
  })
})
