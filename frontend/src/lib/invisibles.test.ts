import { describe, expect, it } from 'vitest'
import { hasInvisibles, revealControlChars, revealInvisibles } from './invisibles'

describe('revealInvisibles', () => {
  it('replaces RLO with a visible token', () => {
    expect(revealInvisibles('a‮b')).toBe('a⟨U+202E⟩b')
  })
  it('replaces zero-width space and BOM', () => {
    expect(revealInvisibles('x​y﻿')).toBe('x⟨U+200B⟩y⟨U+FEFF⟩')
  })
  it('replaces every listed isolate control', () => {
    expect(revealInvisibles('⁦⁧⁨⁩')).toBe('⟨U+2066⟩⟨U+2067⟩⟨U+2068⟩⟨U+2069⟩')
  })
  it('replaces LRM, RLM, and ALM', () => {
    expect(revealInvisibles('a‎‏؜b')).toBe('a⟨U+200E⟩⟨U+200F⟩⟨U+061C⟩b')
  })
  it('leaves normal text (including CJK and emoji) untouched', () => {
    expect(revealInvisibles('日本語 emoji 🎉 tab\t')).toBe('日本語 emoji 🎉 tab\t')
  })
  it('replaces ESC and other C0 controls but keeps TAB literal', () => {
    expect(revealInvisibles('a\x1bb\tc\nd')).toBe('a⟨U+001B⟩b\tc⟨U+000A⟩d')
  })
  it('replaces DEL and C1 controls', () => {
    expect(revealInvisibles('a\x7fb\x9cc')).toBe('a⟨U+007F⟩b⟨U+009C⟩c')
  })
  it('replaces U+2028 and U+2029', () => {
    expect(revealInvisibles('a\u2028b\u2029c')).toBe('a⟨U+2028⟩b⟨U+2029⟩c')
  })
  it('hasInvisibles detects and rejects accordingly', () => {
    expect(hasInvisibles('plain')).toBe(false)
    expect(hasInvisibles('a‮b')).toBe(true)
    expect(hasInvisibles('a\x1bb')).toBe(true)
    expect(hasInvisibles('a\tb')).toBe(true)
  })
})

describe('revealControlChars', () => {
  it('also escapes TAB, unlike revealInvisibles', () => {
    expect(revealControlChars('a\tb')).toBe('a⟨U+0009⟩b')
  })
  it('escapes ESC and LF the same as revealInvisibles', () => {
    expect(revealControlChars('a\x1bb\nc')).toBe('a⟨U+001B⟩b⟨U+000A⟩c')
  })
  it('still escapes bidi/invisible characters', () => {
    expect(revealControlChars('a‮b')).toBe('a⟨U+202E⟩b')
  })
  it('leaves plain text untouched', () => {
    expect(revealControlChars('日本語 emoji 🎉')).toBe('日本語 emoji 🎉')
  })
})
