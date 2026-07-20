import { describe, expect, it } from 'vitest'
import { isVerdictConfirmed } from './confirmation'

describe('isVerdictConfirmed', () => {
  it('is false without a verdict', () => {
    expect(isVerdictConfirmed(null, 0, 0)).toBe(false)
    expect(isVerdictConfirmed(undefined, 3, 3)).toBe(false)
  })

  it('confirms approve without any comment', () => {
    expect(isVerdictConfirmed('approve', 0, 0)).toBe(true)
  })

  it('keeps request-changes and comment unconfirmed until a comment exists', () => {
    expect(isVerdictConfirmed('request-changes', 0, 0)).toBe(false)
    expect(isVerdictConfirmed('comment', 0, 0)).toBe(false)
  })

  it('confirms via a line comment on the concern', () => {
    expect(isVerdictConfirmed('request-changes', 1, 0)).toBe(true)
    expect(isVerdictConfirmed('comment', 2, 0)).toBe(true)
  })

  it('confirms via a general comment', () => {
    expect(isVerdictConfirmed('request-changes', 0, 1)).toBe(true)
    expect(isVerdictConfirmed('comment', 0, 1)).toBe(true)
  })
})
