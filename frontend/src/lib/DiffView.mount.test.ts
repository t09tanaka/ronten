// @vitest-environment jsdom
//
// A real component-mount test, deliberately separate from the pure-function
// tests in state.svelte.test.ts / collapse.test.ts: those tests missed a
// Critical bug because they never actually rendered DiffView/HunkView.
//
// The bug: DiffView's inner `{#each group.refs as hunkRef (hunkRef.hunk ??
// -1)}` keys purely on (file, hunk), not on the selected concern. A hunk
// shared by two concerns therefore kept the SAME HunkView component
// instance across a concern switch — only its props updated, so the
// `collapsed = $state(...)` seed (which HunkView's own comment says is
// "captured only on the initial value") never re-ran. A shared hunk under
// an over-threshold concern could stay expanded, silently escaping the
// force-collapse-when-total-exceeds-1000 rule this feature exists to
// enforce. The fix wraps the per-concern render in
// `{#key rs.selected.id}...{/key}` so switching concerns always remounts.
import { tick } from 'svelte'
import { fireEvent, render } from '@testing-library/svelte'
import { afterEach, describe, expect, it } from 'vitest'
import DiffView from './DiffView.svelte'
import { rs } from './state.svelte'
import type { ConcernView, DiffLine, FileDiff, Hunk, HunkRef, Session } from './types'

afterEach(() => {
  document.body.innerHTML = ''
})

function makeLine(n: number): DiffLine {
  return { kind: 'context', content: `line ${n}`, eol: 'lf', old_no: n, new_no: n }
}

function makeHunk(lineCount: number): Hunk {
  return {
    old_start: 1,
    old_count: lineCount,
    new_start: 1,
    new_count: lineCount,
    section: '',
    lines: Array.from({ length: lineCount }, (_, i) => makeLine(i + 1)),
  }
}

function ref(file: number, hunk: number): HunkRef {
  return { file, hunk }
}

function baseSession(): Omit<Session, 'files' | 'concerns'> {
  return {
    title: 'session',
    summary: null,
    warnings: [],
    draft: { concerns: {}, general_comments: [], acknowledged_opaque: [] },
    draft_revision: 0,
    limits: { max_comments: 500, max_comment_chars: 10_000, max_draft_bytes: 8 * 1024 * 1024 },
    finished: null,
    unmapped_lines: [],
  }
}

/** One file, one hunk, one concern owning it — the minimal fixture for
 * exercising a single hunk's own collapse behavior in isolation. */
function singleHunkSession(lineCount: number): Session {
  const hunk = makeHunk(lineCount)
  const file: FileDiff = {
    old_path: 'a.txt',
    new_path: 'a.txt',
    change_kind: 'modified',
    content_kind: 'text',
    old_mode: null,
    new_mode: null,
    old_type: 'regular',
    new_type: 'regular',
    old_oid: 'a',
    new_oid: 'b',
    old_size: 100,
    new_size: 100,
    lfs_pointer: false,
    hunks: [hunk],
  }
  const concern: ConcernView = {
    id: 'only',
    title: 'Only concern',
    description: null,
    risk: null,
    unmapped: false,
    hunks: [ref(0, 0)],
  }
  return { ...baseSession(), files: [file], concerns: [concern] }
}

const B_ONLY_HUNK_COUNT = 50

/** One file with 51 hunks of 20 lines each: hunk 0 is shared by both
 * concerns below; the other 50 belong only to concern B. Concern A's total
 * (20 lines, just the shared hunk) stays well under the 1,000-line
 * force-collapse threshold; concern B's total (51 x 20 = 1,020) exceeds it —
 * while no single hunk anywhere crosses HunkView's own 200-line per-hunk
 * threshold. */
function makeSession(): Session {
  const sharedHunk = makeHunk(20)
  const bOnlyHunks = Array.from({ length: B_ONLY_HUNK_COUNT }, () => makeHunk(20))
  const file: FileDiff = {
    old_path: 'a.txt',
    new_path: 'a.txt',
    change_kind: 'modified',
    content_kind: 'text',
    old_mode: null,
    new_mode: null,
    old_type: 'regular',
    new_type: 'regular',
    old_oid: 'a',
    new_oid: 'b',
    old_size: 100,
    new_size: 100,
    lfs_pointer: false,
    hunks: [sharedHunk, ...bOnlyHunks],
  }
  const concernA: ConcernView = {
    id: 'A',
    title: 'Concern A',
    description: null,
    risk: null,
    unmapped: false,
    hunks: [ref(0, 0)],
  }
  const concernB: ConcernView = {
    id: 'B',
    title: 'Concern B',
    description: null,
    risk: null,
    unmapped: false,
    hunks: [ref(0, 0), ...bOnlyHunks.map((_, i) => ref(0, i + 1))],
  }
  return {
    title: 'session',
    summary: null,
    files: [file],
    concerns: [concernA, concernB],
    warnings: [],
    draft: { concerns: {}, general_comments: [], acknowledged_opaque: [] },
    draft_revision: 0,
    limits: { max_comments: 500, max_comment_chars: 10_000, max_draft_bytes: 8 * 1024 * 1024 },
    finished: null,
    unmapped_lines: [],
  }
}

describe('DiffView force-collapse survives a concern switch', () => {
  it('an over-threshold concern collapses every hunk, including one shared with an under-threshold concern', async () => {
    rs.session = makeSession()
    rs.selectedIdx = 0 // concern A: only the 20-line shared hunk — well under the threshold
    rs.phase = 'review'

    const { container } = render(DiffView)
    await tick()

    // Under A, the shared hunk is small and A is under threshold: it
    // mounts expanded — a real hunk table, not a collapsed placeholder.
    expect(container.querySelector('.hunk-table')).not.toBeNull()
    expect(container.textContent).not.toContain('click to expand')

    // Switch to concern B, whose 51 hunks (including the SAME shared hunk)
    // total 1,020 rendered lines — over the force-collapse threshold.
    rs.selectedIdx = 1
    await tick()

    // Every one of B's hunks, including the one shared with A, must start
    // collapsed: no hunk table mounted, and one "click to expand" toggle
    // per hunk. This is the assertion that fails without the {#key} fix —
    // the shared hunk (reusing A's HunkView instance) stays expanded.
    expect(container.querySelector('.hunk-table')).toBeNull()
    const expandButtons = [...container.querySelectorAll('.collapse-toggle')].filter((b) =>
      b.textContent?.includes('click to expand'),
    )
    expect(expandButtons).toHaveLength(B_ONLY_HUNK_COUNT + 1)
  })
})

describe('DiffView hunk collapse fundamentals', () => {
  it('a concern whose hunks total over 1000 lines initializes every hunk collapsed on first mount', async () => {
    // 51 x 20 = 1,020 lines, all in one concern — no prior selection/switch
    // involved, isolating the "total exceeds threshold" rule from the
    // remount-on-switch behavior covered above.
    const hunks = Array.from({ length: 51 }, () => makeHunk(20))
    const file: FileDiff = {
      old_path: 'a.txt',
      new_path: 'a.txt',
      change_kind: 'modified',
      content_kind: 'text',
      old_mode: null,
      new_mode: null,
      old_type: 'regular',
      new_type: 'regular',
      old_oid: 'a',
      new_oid: 'b',
      old_size: 100,
      new_size: 100,
      lfs_pointer: false,
      hunks,
    }
    const concern: ConcernView = {
      id: 'big',
      title: 'Big concern',
      description: null,
      risk: null,
      unmapped: false,
      hunks: hunks.map((_, i) => ref(0, i)),
    }
    rs.session = { ...baseSession(), files: [file], concerns: [concern] }
    rs.selectedIdx = 0
    rs.phase = 'review'

    const { container } = render(DiffView)
    await tick()

    expect(container.querySelector('.hunk-table')).toBeNull()
    const expandButtons = [...container.querySelectorAll('.collapse-toggle')].filter((b) =>
      b.textContent?.includes('click to expand'),
    )
    expect(expandButtons).toHaveLength(51)
  })

  it('a single hunk over 200 lines starts collapsed even though its concern is well under the total threshold', async () => {
    rs.session = singleHunkSession(250)
    rs.selectedIdx = 0
    rs.phase = 'review'

    const { container } = render(DiffView)
    await tick()

    expect(container.querySelector('.hunk-table')).toBeNull()
    const toggle = container.querySelector('.collapse-toggle')
    expect(toggle?.textContent).toContain('250 lines — click to expand')
  })

  it('the collapse toggle expands and re-collapses a hunk on click', async () => {
    rs.session = singleHunkSession(250)
    rs.selectedIdx = 0
    rs.phase = 'review'

    const { container } = render(DiffView)
    await tick()

    const expandButton = container.querySelector('.collapse-toggle') as HTMLButtonElement
    expect(expandButton.textContent).toContain('click to expand')
    await fireEvent.click(expandButton)

    expect(container.querySelector('.hunk-table')).not.toBeNull()
    const collapseButton = container.querySelector('.collapse-toggle') as HTMLButtonElement
    expect(collapseButton.textContent).toBe('Collapse')

    await fireEvent.click(collapseButton)

    expect(container.querySelector('.hunk-table')).toBeNull()
    expect(container.querySelector('.collapse-toggle')?.textContent).toContain('click to expand')
  })
})
