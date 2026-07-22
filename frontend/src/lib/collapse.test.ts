import { describe, expect, it } from 'vitest'
import { CONCERN_TOTAL_LINES_COLLAPSE_THRESHOLD, shouldCollapseAllHunks } from './collapse'

/** Sum of `count` hunks each contributing `linesPerHunk` rendered lines —
 * mirrors how DiffView derives `selectedTotalLines` from a concern's real
 * hunks (`hunk.lines.length` summed), without needing full Hunk/FileDiff
 * fixtures just to exercise the pure threshold decision. */
function totalLines(count: number, linesPerHunk: number): number {
  return count * linesPerHunk
}

describe('shouldCollapseAllHunks', () => {
  it('many_small_hunks_collapse_when_total_exceeds_threshold', () => {
    // 1,000 hunks x 20 lines = 20,000 rendered lines. No single hunk is
    // anywhere near HunkView's own 200-line per-hunk threshold, but the
    // concern as a whole would still build 20,000 lines of DOM at once
    // without this rule.
    const total = totalLines(1000, 20)
    expect(total).toBe(20_000)
    expect(shouldCollapseAllHunks(total)).toBe(true)
  })

  it('a concern under the threshold is not force-collapsed', () => {
    // 40 hunks x 20 lines = 800 rendered lines, under the 1,000-line bar.
    const total = totalLines(40, 20)
    expect(total).toBe(800)
    expect(shouldCollapseAllHunks(total)).toBe(false)
  })

  it('is a strict threshold: exactly at the limit does not force-collapse', () => {
    expect(shouldCollapseAllHunks(CONCERN_TOTAL_LINES_COLLAPSE_THRESHOLD)).toBe(false)
    expect(shouldCollapseAllHunks(CONCERN_TOTAL_LINES_COLLAPSE_THRESHOLD + 1)).toBe(true)
  })
})
