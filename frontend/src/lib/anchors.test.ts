import { describe, expect, it } from 'vitest'
import { newTarget, oldTarget } from './anchors'
import type { ChangeKind, DiffLine, FileDiff, LineKind } from './types'

function makeFile(
  old_path: string | null,
  new_path: string | null,
  change_kind: ChangeKind = 'modified',
): FileDiff {
  return {
    id: 'test-file-id',
    old_path,
    new_path,
    change_kind,
    content_kind: 'text',
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
  }
}

function makeLine(kind: LineKind, old_no: number | null, new_no: number | null): DiffLine {
  return { kind, content: 'x', eol: 'lf', old_no, new_no }
}

describe('oldTarget / newTarget', () => {
  const renamed = makeFile('old_name.rs', 'new_name.rs', 'renamed')

  it('anchors a remove line in a renamed file to old_path on the old side', () => {
    const line = makeLine('remove', 27, null)
    expect(oldTarget(renamed, line)).toEqual({ path: 'old_name.rs', side: 'old', line: 27 })
    expect(newTarget(renamed, line)).toBeNull()
  })

  it('anchors an add line in a renamed file to new_path only', () => {
    const line = makeLine('add', null, 30)
    expect(oldTarget(renamed, line)).toBeNull()
    expect(newTarget(renamed, line)).toEqual({ path: 'new_name.rs', side: 'new', line: 30 })
  })

  it('gives a context line both targets, each with its own path and number', () => {
    const line = makeLine('context', 10, 12)
    expect(oldTarget(renamed, line)).toEqual({ path: 'old_name.rs', side: 'old', line: 10 })
    expect(newTarget(renamed, line)).toEqual({ path: 'new_name.rs', side: 'new', line: 12 })
  })

  it('uses the same path on both sides for a non-renamed file', () => {
    const file = makeFile('a.rs', 'a.rs')
    const line = makeLine('context', 5, 6)
    expect(oldTarget(file, line)).toEqual({ path: 'a.rs', side: 'old', line: 5 })
    expect(newTarget(file, line)).toEqual({ path: 'a.rs', side: 'new', line: 6 })
  })

  it('returns null for oldTarget when the file has no old_path (added file)', () => {
    const added = makeFile(null, 'a.rs', 'added')
    const line = makeLine('add', null, 1)
    expect(oldTarget(added, line)).toBeNull()
    expect(newTarget(added, line)).toEqual({ path: 'a.rs', side: 'new', line: 1 })
  })

  it('returns null for newTarget when the file has no new_path (deleted file)', () => {
    const deleted = makeFile('a.rs', null, 'deleted')
    const line = makeLine('remove', 3, null)
    expect(oldTarget(deleted, line)).toEqual({ path: 'a.rs', side: 'old', line: 3 })
    expect(newTarget(deleted, line)).toBeNull()
  })

  it('returns null when the path exists but the line number is missing', () => {
    const file = makeFile('a.rs', 'a.rs')
    expect(oldTarget(file, makeLine('add', null, 4))).toBeNull()
    expect(newTarget(file, makeLine('remove', 4, null))).toBeNull()
  })

  it('returns null on both sides when both path and line number are missing', () => {
    const added = makeFile(null, 'a.rs', 'added')
    expect(oldTarget(added, makeLine('add', null, 9))).toBeNull()
    const deleted = makeFile('a.rs', null, 'deleted')
    expect(newTarget(deleted, makeLine('remove', 9, null))).toBeNull()
  })
})
