import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { Session } from './types'

vi.mock('./api', () => ({
  fetchSession: vi.fn(),
  saveDraft: vi.fn(),
  submit: vi.fn(),
  abortSession: vi.fn(),
}))

import { fetchSession, saveDraft } from './api'
import { ReviewState } from './state.svelte'

const fetchSessionMock = vi.mocked(fetchSession)
const saveDraftMock = vi.mocked(saveDraft)

function makeSession(): Session {
  return {
    title: 'session',
    summary: null,
    files: [],
    concerns: [],
    warnings: [],
    draft: { concerns: {}, general_comments: [], acknowledged_opaque: [] },
    draft_revision: 3,
    limits: { max_comments: 500, max_comment_chars: 10_000, max_draft_bytes: 8 * 1024 * 1024 },
    submitted: false,
    unmapped_lines: [],
  }
}

async function loadedState(): Promise<ReviewState> {
  fetchSessionMock.mockResolvedValue(makeSession())
  const rs = new ReviewState()
  await rs.load()
  return rs
}

beforeEach(() => {
  vi.useFakeTimers()
})

afterEach(() => {
  vi.useRealTimers()
  vi.clearAllMocks()
})

describe('draft revision tracking', () => {
  it('echoes the loaded revision and adopts the returned one', async () => {
    const rs = await loadedState()

    saveDraftMock.mockResolvedValueOnce({ ok: true, revision: 4 })
    rs.addGeneralComment('first')
    await vi.runAllTimersAsync()
    expect(saveDraftMock).toHaveBeenLastCalledWith(rs.draft, 3)
    expect(rs.saveState).toBe('saved')

    saveDraftMock.mockResolvedValueOnce({ ok: true, revision: 5 })
    rs.addGeneralComment('second')
    await vi.runAllTimersAsync()
    expect(saveDraftMock).toHaveBeenLastCalledWith(rs.draft, 4)
  })
})

describe('draft conflict handling', () => {
  it('raises the conflict flag and stops autosave on a draft conflict', async () => {
    const rs = await loadedState()

    saveDraftMock.mockResolvedValueOnce({
      ok: false,
      error: 'draft conflict',
      current_revision: 9,
    })
    rs.addGeneralComment('mine')
    await vi.runAllTimersAsync()
    expect(rs.draftConflict).toBe(true)
    expect(rs.saveState).toBe('error')
    expect(rs.phase).toBe('review')
    expect(saveDraftMock).toHaveBeenCalledTimes(1)

    // No retry until the user reloads: further edits don't schedule saves.
    rs.addGeneralComment('more')
    await vi.runAllTimersAsync()
    expect(saveDraftMock).toHaveBeenCalledTimes(1)
  })

  it('joins the submitted state on an already-submitted conflict', async () => {
    const rs = await loadedState()

    saveDraftMock.mockResolvedValueOnce({ ok: false, error: 'already submitted' })
    rs.addGeneralComment('late')
    await vi.runAllTimersAsync()
    expect(rs.phase).toBe('submitted')
    expect(rs.saveState).toBe('idle')
    expect(rs.draftConflict).toBe(false)
  })
})
