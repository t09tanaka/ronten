import { describe, expect, it } from 'vitest'
import { formatCountdown, remainingMs } from './countdown'

describe('remainingMs', () => {
  it('computes the gap between now and a future deadline', () => {
    const now = Date.parse('2026-07-22T10:00:00Z')
    const deadline = new Date(now + 90_000).toISOString()
    expect(remainingMs(deadline, now)).toBe(90_000)
  })

  it('clamps a past deadline to zero instead of going negative', () => {
    const now = Date.parse('2026-07-22T10:00:00Z')
    const deadline = new Date(now - 5_000).toISOString()
    expect(remainingMs(deadline, now)).toBe(0)
  })

  it('clamps clock-skew (deadline already passed by server-vs-client drift) to zero', () => {
    const now = Date.parse('2026-07-22T10:00:00Z')
    // Deadline computed by a server clock slightly behind the client's.
    const deadline = new Date(now - 1).toISOString()
    expect(remainingMs(deadline, now)).toBe(0)
  })

  it('treats an unparseable deadline as zero remaining rather than throwing', () => {
    expect(remainingMs('not-a-date', Date.now())).toBe(0)
  })
})

describe('formatCountdown', () => {
  it('formats sub-minute durations with zero-padded seconds', () => {
    expect(formatCountdown(5_000)).toBe('0:05')
  })

  it('formats minutes and seconds', () => {
    expect(formatCountdown(90_000)).toBe('1:30')
  })

  it('formats zero as 0:00', () => {
    expect(formatCountdown(0)).toBe('0:00')
  })

  it('does not cap minutes for long durations', () => {
    expect(formatCountdown(61 * 60_000)).toBe('61:00')
  })
})
