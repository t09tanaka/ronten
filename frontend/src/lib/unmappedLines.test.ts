import { describe, expect, it } from 'vitest'
import { buildUnmappedSet, isUnmappedInSet, unmappedKey } from './unmappedLines'

describe('unmappedKey', () => {
  it('joins file, side, and line with colons', () => {
    expect(unmappedKey(2, 'old', 10)).toBe('2:old:10')
    expect(unmappedKey(0, 'new', 1)).toBe('0:new:1')
  })

  it('keeps old and new sides distinct for the same file/line', () => {
    expect(unmappedKey(1, 'old', 5)).not.toBe(unmappedKey(1, 'new', 5))
  })
})

describe('buildUnmappedSet / isUnmappedInSet', () => {
  const set = buildUnmappedSet([
    { file: 0, side: 'new', line: 12 },
    { file: 0, side: 'old', line: 8 },
    { file: 2, side: 'new', line: 3 },
  ])

  it('finds an exact (file, side, line) match', () => {
    expect(isUnmappedInSet(set, 0, 'new', 12)).toBe(true)
    expect(isUnmappedInSet(set, 0, 'old', 8)).toBe(true)
    expect(isUnmappedInSet(set, 2, 'new', 3)).toBe(true)
  })

  it('does not match a different file, side, or line number', () => {
    expect(isUnmappedInSet(set, 1, 'new', 12)).toBe(false)
    expect(isUnmappedInSet(set, 0, 'old', 12)).toBe(false)
    expect(isUnmappedInSet(set, 0, 'new', 13)).toBe(false)
  })

  it('never matches a null line (context lines / missing side)', () => {
    expect(isUnmappedInSet(set, 0, 'new', null)).toBe(false)
    expect(isUnmappedInSet(set, 0, 'old', null)).toBe(false)
  })

  it('returns false for an empty set', () => {
    expect(isUnmappedInSet(buildUnmappedSet([]), 0, 'new', 1)).toBe(false)
  })
})
