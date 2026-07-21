import { describe, expect, it } from 'vitest'
import { contentNote, opaqueDetails } from './opaque'
import type { FileDiff } from './types'

function makeFile(overrides: Partial<FileDiff> = {}): FileDiff {
  return {
    old_path: 'a.bin',
    new_path: 'a.bin',
    change_kind: 'modified',
    content_kind: 'binary',
    old_mode: null,
    new_mode: null,
    old_oid: null,
    new_oid: null,
    old_size: null,
    new_size: null,
    hunks: [],
    ...overrides,
  }
}

describe('contentNote', () => {
  it('returns an empty string for text', () => {
    expect(contentNote('text')).toBe('')
  })

  it('describes a binary file', () => {
    expect(contentNote('binary')).toBe('Binary file changed — contents not displayed')
  })

  it('describes a non-utf8 file', () => {
    expect(contentNote('non-utf8')).toBe('Non-UTF-8 file changed — contents not displayed')
  })

  it('describes a too-large file', () => {
    expect(contentNote('too-large')).toBe(
      'File changed — contents omitted because a display limit was exceeded',
    )
  })
})

describe('opaqueDetails', () => {
  it('produces mode, oid (truncated to 12 chars), and size rows', () => {
    const file = makeFile({
      old_mode: '100644',
      new_mode: '100755',
      old_oid: 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa',
      new_oid: 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb',
      old_size: 10,
      new_size: 20,
    })
    expect(opaqueDetails(file)).toEqual([
      { label: 'mode', value: '100644 → 100755' },
      { label: 'oid', value: 'aaaaaaaaaaaa → bbbbbbbbbbbb' },
      { label: 'size', value: '10 B → 20 B' },
    ])
  })

  it('omits a row entirely when both sides are null', () => {
    const file = makeFile({
      old_mode: null,
      new_mode: null,
      old_oid: null,
      new_oid: null,
      old_size: null,
      new_size: null,
    })
    expect(opaqueDetails(file)).toEqual([])
  })

  it('renders an added file (old side null) as "— → x"', () => {
    const file = makeFile({
      old_mode: null,
      new_mode: '100644',
      old_oid: null,
      new_oid: 'cccccccccccccccccccccccccccccccccccccccc',
      old_size: null,
      new_size: 42,
    })
    expect(opaqueDetails(file)).toEqual([
      { label: 'mode', value: '— → 100644' },
      { label: 'oid', value: '— → cccccccccccc' },
      { label: 'size', value: '— → 42 B' },
    ])
  })

  it('formats sizes with locale thousands separators', () => {
    const file = makeFile({
      old_mode: null,
      new_mode: null,
      old_oid: null,
      new_oid: null,
      old_size: 1234567,
      new_size: null,
    })
    expect(opaqueDetails(file)).toEqual([{ label: 'size', value: '1,234,567 B → —' }])
  })
})
