import { describe, expect, it } from 'vitest'
import { scalarLength, truncateToScalars } from './textLimits'

describe('scalarLength', () => {
  it('char_count_uses_scalars_not_utf16', () => {
    // "😀" is one Unicode scalar value but a UTF-16 surrogate pair (2 code
    // units) — this is exactly the gap between Rust's `chars().count()`
    // (what the server enforces max_comment_chars with) and JS's
    // `String.prototype.length`.
    const emoji = '😀'
    expect(emoji.length).toBe(2)
    expect(scalarLength(emoji)).toBe(1)

    const repeated = emoji.repeat(100)
    expect(repeated.length).toBe(200)
    expect(scalarLength(repeated)).toBe(100)
  })

  it('counts ordinary BMP text the same as .length', () => {
    const s = 'hello world'
    expect(scalarLength(s)).toBe(s.length)
  })
})

describe('truncateToScalars', () => {
  it('leaves a string under the limit untouched', () => {
    expect(truncateToScalars('abc', 10)).toBe('abc')
  })

  it('truncates by scalar count, not UTF-16 units', () => {
    const s = '😀😀😀'
    const truncated = truncateToScalars(s, 2)
    expect(scalarLength(truncated)).toBe(2)
    expect(truncated).toBe('😀😀')
  })

  it('never splits a surrogate pair', () => {
    const truncated = truncateToScalars('😀', 1)
    // A split surrogate pair would produce an unpaired lone surrogate
    // (length 1, invalid as a standalone code unit) instead of the whole
    // 2-unit emoji.
    expect(truncated).toBe('😀')
    expect(truncated.length).toBe(2)
  })
})
