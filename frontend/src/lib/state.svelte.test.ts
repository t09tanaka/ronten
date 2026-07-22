import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { HunkRef, SaveDraftResult, Session } from './types'

vi.mock('./api', () => ({
  fetchSession: vi.fn(),
  saveDraft: vi.fn(),
  submit: vi.fn(),
  abortSession: vi.fn(),
}))

import { abortSession, fetchSession, saveDraft, submit } from './api'
import {
  commentTargetKey,
  GENERAL_BUFFER_KEY,
  phaseForFinished,
  ReviewState,
  SAVE_DEBOUNCE_MS,
} from './state.svelte'

const fetchSessionMock = vi.mocked(fetchSession)
const saveDraftMock = vi.mocked(saveDraft)
const submitMock = vi.mocked(submit)
const abortSessionMock = vi.mocked(abortSession)

/** What `apiFetch`'s AbortController throws when the 40s client timeout
 * fires — the ambiguous case outcome-unknown recovery must react to. */
function abortError(): DOMException {
  return new DOMException('The operation was aborted.', 'AbortError')
}

/** What `fetch` itself rejects with on a network failure (offline, DNS,
 * connection reset) — the other ambiguous case. */
function networkError(): TypeError {
  return new TypeError('Failed to fetch')
}

function makeSession(): Session {
  return {
    title: 'session',
    summary: null,
    files: [],
    concerns: [],
    warnings: [],
    draft: { concerns: {}, general_comments: [], acknowledgements: [] },
    draft_revision: 3,
    limits: {
      max_comments: 500,
      max_comment_chars: 10_000,
      max_total_comments: 1000,
      max_total_comment_chars: 1_500_000,
      max_draft_bytes: 8 * 1024 * 1024,
    },
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

/** Two concerns so `select` can move away from index 0. */
function sessionWithConcerns(): Session {
  return {
    ...makeSession(),
    concerns: [
      { id: 'c1', title: 'Concern 1', description: null, risk: null, unmapped: false, hunks: [] },
      { id: 'c2', title: 'Concern 2', description: null, risk: null, unmapped: false, hunks: [] },
    ],
  }
}

beforeEach(() => {
  vi.useFakeTimers()
})

afterEach(() => {
  vi.useRealTimers()
  vi.clearAllMocks()
  vi.unstubAllGlobals()
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

  it('joins the timed_out state (not aborted) on a finished(timeout) conflict', async () => {
    const rs = await loadedState()

    saveDraftMock.mockResolvedValueOnce({
      ok: false,
      error: 'session finished',
      finished: 'timeout',
    })
    rs.addGeneralComment('late')
    await vi.runAllTimersAsync()
    expect(rs.phase).toBe('timed_out')
    expect(rs.saveState).toBe('idle')
    expect(rs.draftConflict).toBe(false)
  })
})

describe('phaseForFinished', () => {
  it('finished_timeout_maps_to_timed_out_phase', () => {
    // Task 2.4: `timeout` used to fold into the aborted screen; it now gets
    // its own terminal phase so the UI can tell a reviewer who ran out of
    // time apart from one who explicitly cancelled.
    expect(phaseForFinished('timeout')).toBe('timed_out')
  })

  it('finished_aborted_maps_to_aborted_phase', () => {
    expect(phaseForFinished('aborted')).toBe('aborted')
  })

  it('finished_submitted_maps_to_submitted_phase', () => {
    expect(phaseForFinished('submitted')).toBe('submitted')
  })

  it('null_finished_maps_to_review_phase', () => {
    expect(phaseForFinished(null)).toBe('review')
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

  it('save_retry_reuses_identical_payload_under_same_id', async () => {
    const rs = await loadedState()

    // The first attempt is left pending (a controllable deferred) so the
    // store can be mutated to "draft B" before it settles — simulating the
    // user typing more while the request for "draft A" is still in flight.
    // Edits aren't locked during autosave, so this is possible even though
    // #runSave itself never yields between taking its snapshot and firing
    // the first request.
    let rejectFirst: ((e: unknown) => void) | undefined
    saveDraftMock.mockImplementationOnce(
      () =>
        new Promise<SaveDraftResult>((_resolve, reject) => {
          rejectFirst = reject
        }),
    )
    saveDraftMock.mockResolvedValueOnce({ ok: true, revision: 4 })

    rs.addGeneralComment('draft A')
    await vi.advanceTimersByTimeAsync(SAVE_DEBOUNCE_MS)
    expect(saveDraftMock).toHaveBeenCalledTimes(1)
    const [firstDraft, firstRevision, firstId] = saveDraftMock.mock.calls[0]
    expect(firstDraft.general_comments).toEqual(['draft A'])

    // Mutate the store directly to "draft B" (bypassing addGeneralComment's
    // own scheduleSave/debounce, which isn't what's under test here) while
    // the first attempt is still on the wire.
    rs.draft.general_comments.push('draft B')

    // The first attempt's response is lost (network/timeout) even though
    // the server actually applied draft A.
    rejectFirst?.(new Error('network error'))
    await vi.advanceTimersByTimeAsync(0)

    expect(saveDraftMock).toHaveBeenCalledTimes(2)
    const [secondDraft, secondRevision, secondId] = saveDraftMock.mock.calls[1]
    // Same mutation id...
    expect(secondId).toBe(firstId)
    // ...and the SAME payload as the first attempt (draft A) — not the
    // store's current state (draft A + draft B). Same id must mean same
    // bytes, or the server's "already applied" replay silently drops B.
    expect(secondDraft).toEqual(firstDraft)
    expect(secondDraft.general_comments).toEqual(['draft A'])
    expect(secondRevision).toBe(firstRevision)
    expect(rs.saveState).toBe('saved')
    // The retry's success only confirms draft A was persisted; draft B is
    // still live in the store, unsaved, waiting for the next debounced save
    // (a new mutation id) to pick it up — this is not asserted further
    // here, it's the documented, correct follow-up behavior.
    expect(rs.draft.general_comments).toEqual(['draft A', 'draft B'])
  })
})

describe('editor buffers (Task 2.5 / P1-5)', () => {
  it('editor_buffer_survives_concern_switch', async () => {
    fetchSessionMock.mockResolvedValue(sessionWithConcerns())
    const rs = new ReviewState()
    await rs.load()

    rs.setEditorBuffer(GENERAL_BUFFER_KEY, 'in progress')
    rs.setEditorBuffer('src/a.ts:new:5', 'inline draft')

    rs.select(1)

    expect(rs.editorBuffer(GENERAL_BUFFER_KEY)).toBe('in progress')
    expect(rs.editorBuffer('src/a.ts:new:5')).toBe('inline draft')
  })

  it('has_unsaved_changes_includes_nonempty_editor_buffer', async () => {
    const rs = await loadedState()

    expect(rs.hasUnsavedChanges).toBe(false)

    rs.setEditorBuffer(GENERAL_BUFFER_KEY, '   ')
    expect(rs.hasUnsavedChanges).toBe(false)

    rs.setEditorBuffer(GENERAL_BUFFER_KEY, '  hello  ')
    expect(rs.hasUnsavedChanges).toBe(true)

    rs.clearEditorBuffer(GENERAL_BUFFER_KEY)
    expect(rs.hasUnsavedChanges).toBe(false)
  })

  it('submit_blocks_when_editor_buffer_nonempty', async () => {
    const rs = await loadedState()
    rs.setEditorBuffer(GENERAL_BUFFER_KEY, 'unfinished thought')

    // The test environment has no `window`/`confirm` global (vitest runs in
    // node, not jsdom) — vi.stubGlobal installs one that can be swapped
    // between test cases and always cleaned up in afterEach.
    const confirmMock = vi.fn().mockReturnValue(false)
    vi.stubGlobal('confirm', confirmMock)

    await rs.submitReview()
    expect(submitMock).not.toHaveBeenCalled()
    expect(rs.phase).toBe('review')

    confirmMock.mockReturnValue(true)
    submitMock.mockResolvedValueOnce({ ok: true })
    await rs.submitReview()
    expect(submitMock).toHaveBeenCalledTimes(1)
    expect(rs.phase).toBe('submitted')
  })

  it('inline_buffer_is_scoped_per_concern', () => {
    // One hunk line can belong to more than one concern (see hunkOwners),
    // so the inline buffer key must include the concern id — otherwise
    // in-progress text typed under concern A would leak into (and commit
    // under) the editor opened on the same line under concern B.
    const target = { path: 'src/a.ts', side: 'new' as const, line: 5 }
    const keyA = commentTargetKey('concernA', target)
    const keyB = commentTargetKey('concernB', target)
    expect(keyA).not.toBe(keyB)

    const rs = new ReviewState()
    rs.setEditorBuffer(keyA, 'typed under concern A')

    expect(rs.editorBuffer(keyA)).toBe('typed under concern A')
    // Same (path, side, line) under a different concern reads empty.
    expect(rs.editorBuffer(keyB)).toBe('')
  })
})

describe('outcome-unknown recovery', () => {
  it('submit_failure_queries_session_and_recovers_to_submitted', async () => {
    const rs = await loadedState()

    // The submit's fetch itself fails (client timeout/network error, not a
    // clean HTTP error response) — the browser never saw the response, but
    // the server may have already committed it.
    submitMock.mockRejectedValueOnce(networkError())
    // The recovery query finds the session already finished server-side:
    // this tab should join that terminal state rather than report failure.
    fetchSessionMock.mockResolvedValueOnce({ ...makeSession(), finished: 'submitted' })

    await rs.submitReview()

    expect(fetchSessionMock).toHaveBeenCalledTimes(2) // load() + the recovery query
    expect(rs.phase).toBe('submitted')
  })

  it('submit_failure_with_session_query_failing_shows_outcome_unknown', async () => {
    const rs = await loadedState()

    // The 40s client-timeout abort, this time — the other ambiguous case.
    submitMock.mockRejectedValueOnce(abortError())
    // The recovery query also fails — the outcome truly can't be determined
    // from here; a single query, then a banner, no retry loop.
    fetchSessionMock.mockRejectedValueOnce(new Error('network error'))

    await rs.submitReview()

    expect(rs.phase).toBe('outcome_unknown')
    // The local draft must survive so the user can still copy it out.
    expect(rs.draft).toEqual({ concerns: {}, general_comments: [], acknowledgements: [] })
  })

  it('submit_failure_with_session_still_reviewing_shows_retryable_error', async () => {
    const rs = await loadedState()

    submitMock.mockRejectedValueOnce(networkError())
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

  it('save_clean_413_does_not_trigger_outcome_unknown_recovery', async () => {
    const rs = await loadedState()

    // saveDraft throws a plain Error for a clean non-2xx/non-409 response
    // (e.g. a confirmed 413 payload-too-large) — a KNOWN server outcome,
    // not a lost one. Both the original attempt and its one same-id retry
    // hit this, matching #runSave's retry-once-on-any-throw behavior.
    saveDraftMock.mockRejectedValueOnce(new Error('saveDraft failed: 413'))
    saveDraftMock.mockRejectedValueOnce(new Error('saveDraft failed: 413'))

    rs.addGeneralComment('too big maybe')
    await vi.runAllTimersAsync()

    // Recovery must not run for a confirmed HTTP error — no extra
    // GET /session beyond the one `load()` already made, and no
    // outcome_unknown banner for what was actually a clean, known failure.
    expect(fetchSessionMock).toHaveBeenCalledTimes(1) // load() only
    expect(rs.phase).toBe('review')
    expect(rs.saveState).toBe('error')
  })

  it('retrying submit from outcome_unknown actually submits instead of silently no-oping', async () => {
    const rs = await loadedState()

    // Drive phase to outcome_unknown via a save whose both attempts and
    // whose recovery query are all lost.
    saveDraftMock.mockRejectedValueOnce(networkError())
    saveDraftMock.mockRejectedValueOnce(networkError())
    fetchSessionMock.mockRejectedValueOnce(new Error('network error'))
    rs.addGeneralComment('x')
    await vi.runAllTimersAsync()
    expect(rs.phase).toBe('outcome_unknown')

    // Before the fix, submitReview's post-flush guard bailed whenever
    // phase !== 'review', so this would resolve without ever calling
    // submit() — a silent no-op behind a confirm dialog.
    submitMock.mockResolvedValueOnce({ ok: true })
    await rs.submitReview()
    expect(submitMock).toHaveBeenCalledTimes(1)
    expect(rs.phase).toBe('submitted')
  })

  it('restores phase to review when an autosave succeeds while outcome_unknown', async () => {
    const rs = await loadedState()

    saveDraftMock.mockRejectedValueOnce(networkError())
    saveDraftMock.mockRejectedValueOnce(networkError())
    fetchSessionMock.mockRejectedValueOnce(new Error('network error'))
    rs.addGeneralComment('x')
    await vi.runAllTimersAsync()
    expect(rs.phase).toBe('outcome_unknown')

    // A later autosave landing is live proof the session is alive — the
    // sticky banner must clear on its own, without a manual reload.
    saveDraftMock.mockResolvedValueOnce({ ok: true, revision: 5 })
    rs.addGeneralComment('y')
    await vi.runAllTimersAsync()

    expect(rs.phase).toBe('review')
    expect(rs.saveState).toBe('saved')
  })
})

describe('hunkOwners owner index', () => {
  function ref(file: number, hunk: number | null): HunkRef {
    return { file, hunk }
  }

  function sessionWithSharedHunks(): Session {
    return {
      ...makeSession(),
      concerns: [
        {
          id: 'c1',
          title: 'Concern 1',
          description: null,
          risk: null,
          unmapped: false,
          hunks: [ref(0, 0), ref(0, 1)],
        },
        {
          id: 'c2',
          title: 'Concern 2',
          description: null,
          risk: null,
          unmapped: false,
          hunks: [ref(0, 1), ref(1, 0)],
        },
        { id: 'c3', title: 'Concern 3', description: null, risk: null, unmapped: false, hunks: [] },
      ],
    }
  }

  it('owner_index_is_o1_lookup', async () => {
    fetchSessionMock.mockResolvedValue(sessionWithSharedHunks())
    const rs = new ReviewState()
    await rs.load()

    // A hunk owned by exactly one concern.
    expect(rs.hunkOwners(ref(0, 0))).toEqual(['c1'])
    // A hunk shared by two concerns (the whole point of the "shared with"
    // badge this powers) must list both, matching the old filter-based
    // hunkOwners' concern-order semantics.
    expect(rs.hunkOwners(ref(0, 1))).toEqual(['c1', 'c2'])
    expect(rs.hunkOwners(ref(1, 0))).toEqual(['c2'])
    // A ref no concern claims: empty list, never undefined.
    expect(rs.hunkOwners(ref(5, 0))).toEqual([])

    // A fresh load (e.g. recovery/reload) must rebuild the index rather
    // than leave stale owners from the previous session around.
    fetchSessionMock.mockResolvedValue({
      ...makeSession(),
      concerns: [
        { id: 'd1', title: 'D1', description: null, risk: null, unmapped: false, hunks: [ref(0, 0)] },
      ],
    })
    await rs.load()
    expect(rs.hunkOwners(ref(0, 0))).toEqual(['d1'])
    expect(rs.hunkOwners(ref(0, 1))).toEqual([])
  })

  it('hunkOwners on an unloaded store returns an empty list, not a throw', () => {
    const rs = new ReviewState()
    expect(rs.hunkOwners(ref(0, 0))).toEqual([])
  })
})

// P1-6: the draft's wire byte size (JSON.stringify + UTF-8 bytes) must be
// tracked against limits.max_draft_bytes so the UI can warn/block before a
// draft grows past what PUT /draft will now refuse server-side (see
// resource_cap_violations in session.rs). A tiny max_draft_bytes here makes
// the thresholds reachable with a couple of ordinary-sized comments instead
// of megabytes of fixture data.
describe('draft byte-size limits (P1-6)', () => {
  async function loadedStateWithByteLimit(maxBytes: number): Promise<ReviewState> {
    fetchSessionMock.mockResolvedValue({
      ...makeSession(),
      limits: { ...makeSession().limits, max_draft_bytes: maxBytes },
    })
    const rs = new ReviewState()
    await rs.load()
    return rs
  }

  // With max_draft_bytes = 320: an empty draft serializes to 59 bytes, a
  // single 230-char general comment to 291 bytes (>= 90% = 288, < 320 —
  // warning without blocking), and a single 260-char comment to 321 bytes
  // (>= 320 — blocked). These exact sizes are JSON.stringify + TextEncoder
  // output, not tuned constants — see the byte-size getters' doc comments
  // in state.svelte.ts for what's actually measured.
  const BYTE_LIMIT = 320

  it('byte_measure_warns_at_90_percent', async () => {
    const rs = await loadedStateWithByteLimit(BYTE_LIMIT)

    // An empty draft's JSON is well under 90% of the cap.
    expect(rs.draftByteWarning).toBe(false)

    rs.addGeneralComment('x'.repeat(230))
    expect(rs.draftByteSize).toBeGreaterThanOrEqual(BYTE_LIMIT * 0.9)
    expect(rs.draftByteSize).toBeLessThan(BYTE_LIMIT)
    expect(rs.draftByteWarning).toBe(true)
    expect(rs.draftByteBlocked).toBe(false)
  })

  it('draft_byte_size_blocks_at_limit', async () => {
    const rs = await loadedStateWithByteLimit(BYTE_LIMIT)

    rs.addGeneralComment('x'.repeat(260))
    expect(rs.draftByteSize).toBeGreaterThanOrEqual(BYTE_LIMIT)
    expect(rs.draftByteBlocked).toBe(true)

    // Further growth is refused once blocked — both a new general comment
    // and a new concern comment.
    const countBefore = rs.draft.general_comments.length
    rs.addGeneralComment('one more comment')
    expect(rs.draft.general_comments.length).toBe(countBefore)

    rs.addComment('c1', { path: 'a.ts', side: 'new', line: 1, body: 'blocked too' })
    expect(rs.draft.concerns.c1).toBeUndefined()
  })

  it('removing content un-blocks further additions', async () => {
    const rs = await loadedStateWithByteLimit(BYTE_LIMIT)
    rs.addGeneralComment('x'.repeat(260))
    expect(rs.draftByteBlocked).toBe(true)

    rs.removeGeneralComment(0)
    expect(rs.draftByteBlocked).toBe(false)
    rs.addGeneralComment('short')
    expect(rs.draft.general_comments).toEqual(['short'])
  })

  it('totalCommentCount and totalCommentChars sum concern and general comments using scalar counts', async () => {
    const rs = await loadedStateWithByteLimit(8 * 1024 * 1024)
    rs.addGeneralComment('😀😀')
    rs.addComment('c1', { path: 'a.ts', side: 'new', line: 1, body: 'abc' })

    expect(rs.totalCommentCount).toBe(2)
    // '😀😀' is 2 Unicode scalars (4 UTF-16 units) + 'abc' is 3 scalars.
    expect(rs.totalCommentChars).toBe(5)
  })
})

// `max_total_comments`/`max_total_comment_chars` can be hit far below
// `max_draft_bytes` (1000 short comments is nowhere near 8 MiB); without a
// client-side gate on these too, the first sign was a failing autosave.
describe('total comment/char caps', () => {
  it('add_blocked_at_total_comment_cap', async () => {
    const rs = await loadedState()
    for (let i = 0; i < rs.limits!.max_total_comments; i++) rs.addGeneralComment(`c${i}`)

    expect(rs.totalCommentCount).toBe(rs.limits!.max_total_comments)
    expect(rs.totalCapBlocked).toBe(true)

    rs.addGeneralComment('one more')
    expect(rs.totalCommentCount).toBe(rs.limits!.max_total_comments)
    rs.addComment('c1', { path: 'a.ts', side: 'new', line: 1, body: 'blocked too' })
    expect(rs.draft.concerns.c1).toBeUndefined()
  })

  it('total_cap_warning_at_90_percent', async () => {
    const rs = await loadedState()
    const ninety = Math.ceil(rs.limits!.max_total_comments * 0.9)
    for (let i = 0; i < ninety - 1; i++) rs.addGeneralComment(`c${i}`)

    expect(rs.totalCapWarning).toBe(false)
    expect(rs.totalCapBlocked).toBe(false)

    rs.addGeneralComment('one more')
    expect(rs.totalCommentCount).toBe(ninety)
    expect(rs.totalCapWarning).toBe(true)
    expect(rs.totalCapBlocked).toBe(false)
  })
})
