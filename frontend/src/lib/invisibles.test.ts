import { describe, expect, it } from 'vitest'
import { hasInvisibles, revealInvisibles } from './invisibles'

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
  it('hasInvisibles detects and rejects accordingly', () => {
    expect(hasInvisibles('plain')).toBe(false)
    expect(hasInvisibles('a‮b')).toBe(true)
  })
})
