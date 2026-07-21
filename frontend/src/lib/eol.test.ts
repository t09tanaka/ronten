import { describe, expect, it } from 'vitest'
import { NO_NEWLINE_MARKER, showCrlfBadge, showNoNewlineMarker } from './eol'
import type { DiffLine, Eol } from './types'

function makeLine(eol: Eol): DiffLine {
  return { kind: 'add', content: 'x', eol, old_no: null, new_no: 1 }
}

describe('showCrlfBadge', () => {
  it('is true only for crlf lines', () => {
    expect(showCrlfBadge(makeLine('crlf'))).toBe(true)
    expect(showCrlfBadge(makeLine('lf'))).toBe(false)
    // A final line without a trailing newline has no line ending to badge.
    expect(showCrlfBadge(makeLine('none'))).toBe(false)
  })
})

describe('showNoNewlineMarker', () => {
  it('is true only for lines with no trailing newline', () => {
    expect(showNoNewlineMarker(makeLine('none'))).toBe(true)
    expect(showNoNewlineMarker(makeLine('lf'))).toBe(false)
    expect(showNoNewlineMarker(makeLine('crlf'))).toBe(false)
  })
})

describe('NO_NEWLINE_MARKER', () => {
  it('matches the unified diff convention', () => {
    expect(NO_NEWLINE_MARKER).toBe('\\ No newline at end of file')
  })
})
