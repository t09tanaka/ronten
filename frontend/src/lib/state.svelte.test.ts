import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { SaveDraftResult, Session } from './types'

vi.mock('./api', () => ({
  fetchSession: vi.fn(),
  saveDraft: vi.fn(),
  submit: vi.fn(),
  abortSession: vi.fn(),
}))

import { abortSession, fetchSession, saveDraft, submit } from './api'
import { ReviewState, SAVE_DEBOUNCE_MS } from './state.svelte'

const fetchSessionMock = vi.mocked(fetchSession)
const saveDraftMock = vi.mocked(saveDraft)
const submitMock = vi.mocked(submit)
const abortSessionMock = vi.mocked(abortSession)

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
    finished: null,
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
    expect(saveDraftMock).toHaveBeenLastCalledWith(rs.draft, 3, expect.any(String))
    expect(rs.saveState).toBe('saved')

    saveDraftMock.mockResolvedValueOnce({ ok: true, revision: 5 })
    rs.addGeneralComment('second')
    await vi.runAllTimersAsync()
    expect(saveDraftMock).toHaveBeenLastCalledWith(rs.draft, 4, expect.any(String))
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

  it('refuses to submit once a draft conflict is standing', async () => {
    const rs = await loadedState()

    saveDraftMock.mockResolvedValueOnce({
      ok: false,
      error: 'draft conflict',
      current_revision: 9,
    })
    rs.addGeneralComment('mine')
    await vi.runAllTimersAsync()
    expect(rs.draftConflict).toBe(true)

    await rs.submitReview()
    expect(submitMock).not.toHaveBeenCalled()
  })

  it('raises the conflict banner when the submit itself hits a stale revision', async () => {
    const rs = await loadedState()

    submitMock.mockResolvedValueOnce({
      error: 'draft conflict',
      current_revision: 9,
    })
    await rs.submitReview()
    expect(rs.draftConflict).toBe(true)
    expect(rs.phase).toBe('review')
  })

  it('joins the submitted state on a finished(submitted) conflict', async () => {
    const rs = await loadedState()

    saveDraftMock.mockResolvedValueOnce({
      ok: false,
      error: 'session finished',
      finished: 'submitted',
    })
    rs.addGeneralComment('late')
    await vi.runAllTimersAsync()
    expect(rs.phase).toBe('submitted')
    expect(rs.saveState).toBe('idle')
    expect(rs.draftConflict).toBe(false)
  })

  it('joins the aborted state on a finished(aborted) conflict', async () => {
    const rs = await loadedState()

    saveDraftMock.mockResolvedValueOnce({
      ok: false,
      error: 'session finished',
      finished: 'aborted',
    })
    rs.addGeneralComment('late')
    await vi.runAllTimersAsync()
    expect(rs.phase).toBe('aborted')
    expect(rs.saveState).toBe('idle')
    expect(rs.draftConflict).toBe(false)
  })
})

describe('mutation serialization', () => {
  it('submit_waits_for_inflight_save_and_sends_latest_draft', async () => {
    const rs = await loadedState()

    // Draft A: an edit fires the debounce timer and the resulting save is
    // left pending (a controllable deferred), simulating "autosave started
    // but hasn't landed yet".
    const saveGate: { resolve: ((v: SaveDraftResult) => void) | null } = { resolve: null }
    saveDraftMock.mockImplementationOnce(
      () =>
        new Promise<SaveDraftResult>((resolve) => {
          saveGate.resolve = resolve
        }),
    )
    rs.addGeneralComment('draft A')
    await vi.advanceTimersByTimeAsync(SAVE_DEBOUNCE_MS)
    expect(saveDraftMock).toHaveBeenCalledTimes(1)
    expect(saveDraftMock).toHaveBeenLastCalledWith(rs.draft, 3, expect.any(String))

    // Draft B: a further edit while that save is still in flight.
    rs.addGeneralComment('draft B')

    // Submit fires while the autosave from draft A is still pending.
    let capturedRevision: number | undefined
    let capturedComments: string[] | undefined
    let capturedMutationId: string | undefined
    submitMock.mockImplementationOnce((draft, revision, mutationId) => {
      capturedRevision = revision
      capturedComments = [...draft.general_comments]
      capturedMutationId = mutationId
      return Promise.resolve({ ok: true })
    })
    const submitPromise = rs.submitReview()

    // Submit must not fire yet — it has to wait for the in-flight save.
    await Promise.resolve()
    await Promise.resolve()
    expect(submitMock).not.toHaveBeenCalled()

    // The in-flight save (draft A, rev 3) now lands, advancing the server's
    // revision to 4.
    saveGate.resolve?.({ ok: true, revision: 4 })
    await submitPromise

    expect(submitMock).toHaveBeenCalledTimes(1)
    expect(capturedRevision).toBe(4)
    expect(capturedComments).toEqual(['draft A', 'draft B'])
    expect(capturedMutationId).toEqual(expect.any(String))
    expect(rs.phase).toBe('submitted')
  })

  it('waits for an in-flight save before aborting, instead of racing it', async () => {
    const rs = await loadedState()

    const saveGate: { resolve: ((v: SaveDraftResult) => void) | null } = { resolve: null }
    saveDraftMock.mockImplementationOnce(
      () =>
        new Promise<SaveDraftResult>((resolve) => {
          saveGate.resolve = resolve
        }),
    )
    rs.addGeneralComment('draft A')
    await vi.advanceTimersByTimeAsync(SAVE_DEBOUNCE_MS)
    expect(saveDraftMock).toHaveBeenCalledTimes(1)

    abortSessionMock.mockResolvedValueOnce(undefined)
    const abortPromise = rs.abortReview()

    // Abort must not fire while the save is still in flight.
    await Promise.resolve()
    await Promise.resolve()
    expect(abortSessionMock).not.toHaveBeenCalled()

    saveGate.resolve?.({ ok: true, revision: 4 })
    await abortPromise

    expect(abortSessionMock).toHaveBeenCalledTimes(1)
    expect(rs.phase).toBe('aborted')
  })
})

describe('mutation id idempotency', () => {
  it('retries a lost save response once, reusing the same mutation id', async () => {
    const rs = await loadedState()

    // The first attempt is a lost response (network/timeout): saveDraft
    // throws. Nothing about the draft changes between the two calls (no
    // `await` in #runSave yields to anything that could edit it), so the
    // retry must resend the exact same draft/revision under the SAME
    // mutation id — that's what lets the server tell it apart from a
    // genuinely new save.
    saveDraftMock.mockRejectedValueOnce(new Error('network error'))
    saveDraftMock.mockResolvedValueOnce({ ok: true, revision: 4 })

    rs.addGeneralComment('once')
    await vi.runAllTimersAsync()

    expect(saveDraftMock).toHaveBeenCalledTimes(2)
    const [firstCall, secondCall] = saveDraftMock.mock.calls
    expect(firstCall[2]).toEqual(expect.any(String))
    expect(secondCall[2]).toBe(firstCall[2])
    expect(secondCall[0]).toEqual(firstCall[0])
    expect(secondCall[1]).toBe(firstCall[1])
    expect(rs.saveState).toBe('saved')
    expect(rs.draftConflict).toBe(false)
  })

  it('mints a fresh mutation id for the next, unrelated save', async () => {
    const rs = await loadedState()

    saveDraftMock.mockResolvedValueOnce({ ok: true, revision: 4 })
    rs.addGeneralComment('first')
    await vi.runAllTimersAsync()
    const firstId = saveDraftMock.mock.calls[0][2]

    saveDraftMock.mockResolvedValueOnce({ ok: true, revision: 5 })
    rs.addGeneralComment('second')
    await vi.runAllTimersAsync()
    const secondId = saveDraftMock.mock.calls[1][2]

    expect(secondId).not.toBe(firstId)
  })
})

describe('outcome-unknown recovery', () => {
  it('submit_failure_queries_session_and_recovers_to_submitted', async () => {
    const rs = await loadedState()

    // The submit's fetch itself fails (client timeout/network error, not a
    // clean HTTP error response) — the browser never saw the response, but
    // the server may have already committed it.
    submitMock.mockRejectedValueOnce(new Error('network error'))
    // The recovery query finds the session already finished server-side:
    // this tab should join that terminal state rather than report failure.
    fetchSessionMock.mockResolvedValueOnce({ ...makeSession(), finished: 'submitted' })

    await rs.submitReview()

    expect(fetchSessionMock).toHaveBeenCalledTimes(2) // load() + the recovery query
    expect(rs.phase).toBe('submitted')
  })

  it('submit_failure_with_session_query_failing_shows_outcome_unknown', async () => {
    const rs = await loadedState()

    submitMock.mockRejectedValueOnce(new Error('network error'))
    // The recovery query also fails — the outcome truly can't be determined
    // from here; a single query, then a banner, no retry loop.
    fetchSessionMock.mockRejectedValueOnce(new Error('network error'))

    await rs.submitReview()

    expect(rs.phase).toBe('outcome_unknown')
    // The local draft must survive so the user can still copy it out.
    expect(rs.draft).toEqual({ concerns: {}, general_comments: [], acknowledged_opaque: [] })
  })

  it('submit_failure_with_session_still_reviewing_shows_retryable_error', async () => {
    const rs = await loadedState()

    submitMock.mockRejectedValueOnce(new Error('network error'))
    fetchSessionMock.mockResolvedValueOnce(makeSession()) // finished: null — still reviewing

    await rs.submitReview()

    expect(rs.phase).toBe('review')
    expect(rs.submitError).toMatch(/retry/i)

    // Idempotent per Task 2.2: a retry after outcome-unknown/retryable-error
    // is safe, so the action must not still be locked.
    submitMock.mockResolvedValueOnce({ ok: true })
    await rs.submitReview()
    expect(rs.phase).toBe('submitted')
  })
})
