import { describe, expect, it } from 'vitest'
import {
  ackReasonLabels,
  contentNote,
  fileNotices,
  modeChangeBadge,
  oneSidedModeBadge,
  opaqueDetails,
  typeChangeBadge,
} from './opaque'
import type { FileDiff } from './types'

function makeFile(overrides: Partial<FileDiff> = {}): FileDiff {
  return {
    id: 'file-id',
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
    ack_required: false,
    ack_reasons: [],
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

// ack_required/ack_reasons are server-computed (FileDiff::ack_reasons in
// gitdiff.rs) and sent verbatim in the payload — the frontend must not
// recompute this policy (P0-5). These tests only exercise the presentation
// helpers that turn the server's reasons into UI text.
describe('ackReasonLabels', () => {
  it('is empty when the file has no ack reasons', () => {
    expect(ackReasonLabels(makeFile({ ack_reasons: [] }))).toEqual([])
  })

  it('renders a human label per server-supplied reason, in order', () => {
    expect(
      ackReasonLabels(makeFile({ ack_reasons: ['opaque-content', 'mode-changed'] })),
    ).toEqual([
      'Content not rendered — the summary below is all that is shown',
      'File mode changed',
    ])
  })

  it('renders a label for every AckReason variant', () => {
    expect(
      ackReasonLabels(
        makeFile({
          ack_reasons: [
            'opaque-content',
            'gitlink-changed',
            'mode-changed',
            'added-symlink',
            'deleted-symlink',
            'added-executable',
            'regular-to-symlink',
            'lfs-pointer',
            'submodule-pointer',
          ],
        }),
      ),
    ).toHaveLength(9)
  })
})

describe('oneSidedModeBadge', () => {
  it('renders "added symlink <mode>" for a newly added symlink', () => {
    expect(
      oneSidedModeBadge(
        makeFile({ change_kind: 'added', new_type: 'symlink', new_mode: '120000' }),
      ),
    ).toBe('added symlink 120000')
  })

  it('renders "added executable <mode>" for a newly added executable', () => {
    expect(
      oneSidedModeBadge(
        makeFile({ change_kind: 'added', new_type: 'executable', new_mode: '100755' }),
      ),
    ).toBe('added executable 100755')
  })

  it('renders "deleted symlink <mode>" for a deleted symlink', () => {
    expect(
      oneSidedModeBadge(
        makeFile({ change_kind: 'deleted', old_type: 'symlink', old_mode: '120000' }),
      ),
    ).toBe('deleted symlink 120000')
  })

  it('is null for a plain added/deleted regular file', () => {
    expect(
      oneSidedModeBadge(
        makeFile({ change_kind: 'added', new_type: 'regular', new_mode: '100644' }),
      ),
    ).toBeNull()
    expect(
      oneSidedModeBadge(
        makeFile({ change_kind: 'deleted', old_type: 'regular', old_mode: '100644' }),
      ),
    ).toBeNull()
  })

  it('is null for a two-sided change (modified)', () => {
    expect(
      oneSidedModeBadge(
        makeFile({ change_kind: 'modified', old_type: 'regular', new_type: 'symlink' }),
      ),
    ).toBeNull()
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

// fileNotices is derived purely from the server-supplied `ack_reasons` list
// now (not recomputed from old_type/old_oid) — these tests exercise that
// filtering/mapping, not the ack policy itself (see gitdiff.rs for that).
describe('fileNotices', () => {
  it('is empty for an ordinary file with no ack reasons', () => {
    expect(fileNotices(makeFile({ ack_reasons: [] }))).toEqual([])
  })

  it('is empty for ack reasons that already have their own header badge (opaque/mode/symlink/executable)', () => {
    expect(
      fileNotices(
        makeFile({
          ack_reasons: [
            'opaque-content',
            'mode-changed',
            'added-symlink',
            'deleted-symlink',
            'added-executable',
            'regular-to-symlink',
          ],
        }),
      ),
    ).toEqual([])
  })

  it('flags a gitlink pointer change', () => {
    expect(fileNotices(makeFile({ ack_reasons: ['gitlink-changed'] }))).toEqual([
      'Submodule pointer change — nested diff not shown',
    ])
  })

  it('flags a submodule reference added or removed', () => {
    expect(fileNotices(makeFile({ ack_reasons: ['submodule-pointer'] }))).toEqual([
      'Submodule reference added or removed — nested diff not shown',
    ])
  })

  it('flags an LFS pointer', () => {
    expect(fileNotices(makeFile({ ack_reasons: ['lfs-pointer'] }))).toEqual([
      'Git LFS pointer — actual content not shown',
    ])
  })

  it('stacks notices, in server-supplied order', () => {
    expect(
      fileNotices(makeFile({ ack_reasons: ['gitlink-changed', 'lfs-pointer'] })),
    ).toEqual([
      'Submodule pointer change — nested diff not shown',
      'Git LFS pointer — actual content not shown',
    ])
  })
})
