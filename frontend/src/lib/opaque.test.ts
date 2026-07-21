import { describe, expect, it } from 'vitest'
import { contentNote, fileNotices, modeChangeBadge, opaqueDetails, requiresAck, typeChangeBadge } from './opaque'
import type { FileDiff } from './types'

function makeFile(overrides: Partial<FileDiff> = {}): FileDiff {
  return {
    old_path: 'a.bin',
    new_path: 'a.bin',
    change_kind: 'modified',
    content_kind: 'binary',
    old_mode: null,
    new_mode: null,
    old_type: null,
    new_type: null,
    old_oid: null,
    new_oid: null,
    old_size: null,
    new_size: null,
    lfs_pointer: false,
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

// Must mirror the server's FileDiff::requires_ack() (gitdiff.rs) exactly —
// validate_draft rejects submissions with missing acks under this condition.
describe('requiresAck', () => {
  it('requires an ack for opaque content kinds', () => {
    expect(requiresAck(makeFile({ content_kind: 'binary' }))).toBe(true)
    expect(requiresAck(makeFile({ content_kind: 'non-utf8' }))).toBe(true)
    expect(requiresAck(makeFile({ content_kind: 'too-large' }))).toBe(true)
  })

  it('does not require an ack for a plain text change', () => {
    expect(requiresAck(makeFile({ content_kind: 'text' }))).toBe(false)
  })

  it('requires an ack when either side is a gitlink', () => {
    expect(
      requiresAck(makeFile({ content_kind: 'text', old_type: 'gitlink', new_type: 'gitlink' })),
    ).toBe(true)
    expect(requiresAck(makeFile({ content_kind: 'text', old_type: 'gitlink' }))).toBe(true)
    expect(requiresAck(makeFile({ content_kind: 'text', new_type: 'gitlink' }))).toBe(true)
  })

  it('requires an ack for a mode change with both sides present', () => {
    expect(
      requiresAck(makeFile({ content_kind: 'text', old_mode: '100644', new_mode: '100755' })),
    ).toBe(true)
    expect(
      requiresAck(makeFile({ content_kind: 'text', old_mode: '100644', new_mode: '120000' })),
    ).toBe(true)
  })

  it('does not require an ack when the mode is unchanged or one-sided', () => {
    expect(
      requiresAck(makeFile({ content_kind: 'text', old_mode: '100644', new_mode: '100644' })),
    ).toBe(false)
    // Added/deleted files have only one side; their kind is already the
    // headline of the change.
    expect(requiresAck(makeFile({ content_kind: 'text', new_mode: '100755' }))).toBe(false)
    expect(requiresAck(makeFile({ content_kind: 'text', old_mode: '100755' }))).toBe(false)
  })
})

describe('modeChangeBadge', () => {
  it('renders "mode old → new" when both sides differ', () => {
    expect(modeChangeBadge(makeFile({ old_mode: '100644', new_mode: '100755' }))).toBe(
      'mode 100644 → 100755',
    )
  })

  it('is null for an unchanged or one-sided mode', () => {
    expect(modeChangeBadge(makeFile({ old_mode: '100644', new_mode: '100644' }))).toBeNull()
    expect(modeChangeBadge(makeFile({ new_mode: '100644' }))).toBeNull()
    expect(modeChangeBadge(makeFile())).toBeNull()
  })
})

describe('typeChangeBadge', () => {
  it('renders "old → new" when both sides differ', () => {
    expect(typeChangeBadge(makeFile({ old_type: 'regular', new_type: 'symlink' }))).toBe(
      'regular → symlink',
    )
  })

  it('is null for an unchanged or one-sided type', () => {
    expect(typeChangeBadge(makeFile({ old_type: 'regular', new_type: 'regular' }))).toBeNull()
    expect(typeChangeBadge(makeFile({ new_type: 'symlink' }))).toBeNull()
  })
})

describe('fileNotices', () => {
  it('is empty for an ordinary file', () => {
    expect(fileNotices(makeFile())).toEqual([])
  })

  it('flags a submodule pointer change when either side is a gitlink', () => {
    expect(fileNotices(makeFile({ old_type: 'gitlink' }))).toEqual([
      'Submodule pointer change — nested diff not shown',
    ])
    expect(fileNotices(makeFile({ new_type: 'gitlink' }))).toEqual([
      'Submodule pointer change — nested diff not shown',
    ])
  })

  it('flags an LFS pointer', () => {
    expect(fileNotices(makeFile({ lfs_pointer: true }))).toEqual([
      'Git LFS pointer — actual content not shown',
    ])
  })

  it('stacks both notices for an LFS-pointer gitlink combination', () => {
    expect(fileNotices(makeFile({ old_type: 'gitlink', lfs_pointer: true }))).toEqual([
      'Submodule pointer change — nested diff not shown',
      'Git LFS pointer — actual content not shown',
    ])
  })
})
