// Single shared review store (Svelte 5 runes).

import { SvelteMap } from 'svelte/reactivity'
import { abortSession, fetchSession, saveDraft, submit } from './api'
import { isVerdictConfirmed } from './confirmation'
import { requiresAck } from './opaque'
import type {
  Comment,
  ConcernDraft,
  ConcernView,
  Draft,
  FinishedKind,
  HunkRef,
  SaveDraftResult,
  Session,
  Side,
  Verdict,
} from './types'
import { buildUnmappedSet, isUnmappedInSet } from './unmappedLines'

export const SAVE_DEBOUNCE_MS = 500

export type Phase =
  | 'loading'
  | 'review'
  | 'submitted'
  | 'aborted'
  | 'timed_out'
  | 'error'
  | 'outcome_unknown'

/** True only for the errors that mean "we genuinely don't know whether this
 * reached the server": the AbortController firing on `apiFetch`'s 40s
 * timeout (a `DOMException` named `AbortError`) or a network failure (fetch
 * itself rejects with a `TypeError` — offline, DNS failure, connection
 * reset). A clean HTTP error response is a KNOWN outcome, not an ambiguous
 * one, and must not trigger outcome-unknown recovery: `submit()` never
 * throws for one (it parses the JSON body regardless of status), and while
 * `saveDraft()` does throw a plain `Error` for a handful of non-2xx/non-409
 * statuses (e.g. a clean 413/422), that's still a confirmed server
 * response, not a lost one. */
function isAmbiguousFetchError(e: unknown): boolean {
  return (e instanceof DOMException && e.name === 'AbortError') || e instanceof TypeError
}

/** UI phase for a server-reported ending. `timeout` gets its own terminal
 * screen distinct from an explicit abort, so a reviewer who ran out of time
 * isn't told they cancelled the review themselves. */
export function phaseForFinished(finished: FinishedKind | null): Phase {
  if (finished === 'submitted') return 'submitted'
  if (finished === 'aborted') return 'aborted'
  if (finished === 'timeout') return 'timed_out'
  return 'review'
}
export type SaveState = 'idle' | 'saving' | 'saved' | 'error'

export interface CommentTarget {
  path: string
  side: Side
  line: number
}

/** Key for the general-comment box's entry in `editorBuffers`. */
export const GENERAL_BUFFER_KEY = 'general'

/** Key for an inline comment editor's entry in `editorBuffers` — one per
 * (concernId, path, side, line). The concernId is required because a single
 * hunk line can belong to more than one concern (see `hunkOwners`): without
 * it, in-progress text typed under concern A would appear in — and commit
 * under — the editor opened on the same line under concern B. */
export function commentTargetKey(concernId: string, t: CommentTarget): string {
  return `${concernId}:${t.path}:${t.side}:${t.line}`
}

export class ReviewState {
  session = $state<Session | null>(null)
  draft = $state<Draft>({ concerns: {}, general_comments: [], acknowledged_opaque: [] })
  selectedIdx = $state(0)
  phase = $state<Phase>('loading')
  submitting = $state(false)
  submitError = $state<string | null>(null)
  pendingCommentTarget = $state<CommentTarget | null>(null)
  /** In-progress comment text keyed by `GENERAL_BUFFER_KEY` (the general
   * comment box) or `commentTargetKey(...)` (an inline comment editor).
   * Lives here rather than in component-local state so it survives concern
   * switches and editor open/close — only a committed `addComment`/
   * `addGeneralComment` (or an explicit discard) clears an entry. A
   * `SvelteMap` (not a plain `$state`-wrapped `Map`) is required for
   * fine-grained reactivity: Svelte 5's `$state` proxy only recurses into
   * plain objects/arrays, not built-ins like `Map`. */
  editorBuffers = new SvelteMap<string, string>()
  saveState = $state<SaveState>('idle')
  /** True once a save was refused because another tab saved first. Saving
   * stays off until the user reloads — retrying would overwrite that tab's
   * draft. A persistent banner tells the user to reload. */
  draftConflict = $state(false)

  #saveTimer: ReturnType<typeof setTimeout> | null = null
  /** Revision the server holds for our draft; echoed on every PUT and
   * replaced with the one the server returns on success. */
  #revision = 0
  /** Single serialization point for every server mutation (save PUT, submit,
   * abort): each is chained after everything already queued, so at most one
   * is ever in flight and each reads `this.draft`/`this.#revision` only once
   * its turn actually arrives — never a stale snapshot from when it was
   * scheduled. This is what closes the P0-3 race: Submit no longer races an
   * in-flight autosave from this same tab. */
  #mutationChain: Promise<void> = Promise.resolve()
  /** True while a submit or abort is in flight, additional to the
   * phase-based lock below — it closes the window between "user clicked
   * Submit/Abort" and "the server answered" during which further edits
   * could be silently left out of what's actually being submitted/aborted. */
  #actionLocked = false

  /** Server-side validation limits (null until the session has loaded). */
  get limits() {
    return this.session?.limits ?? null
  }

  get selected(): ConcernView | null {
    return this.session?.concerns[this.selectedIdx] ?? null
  }

  // Only built while the `_unmapped` concern is selected — everywhere else
  // isUnmappedLine short-circuits to false without touching the session's
  // (possibly large) unmapped_lines list.
  #unmappedSet = $derived.by((): Set<string> | null => {
    if (!this.session || !this.selected?.unmapped) return null
    return buildUnmappedSet(this.session.unmapped_lines)
  })

  /** Whether (file, side, line) is one of the changed lines no concern
   * claimed — only ever true while the `_unmapped` concern is selected. */
  isUnmappedLine(file: number, side: Side, line: number | null): boolean {
    if (!this.#unmappedSet) return false
    return isUnmappedInSet(this.#unmappedSet, file, side, line)
  }

  /** A concern counts as reviewed only once its verdict is confirmed —
   * approve immediately, request-changes/comment once a comment exists
   * (a line comment on the concern or a general comment). */
  isConfirmed(id: string): boolean {
    const cd = this.draft.concerns[id]
    return isVerdictConfirmed(cd?.verdict, cd?.comments.length ?? 0, this.draft.general_comments.length)
  }

  get reviewedCount(): number {
    if (!this.session) return 0
    return this.session.concerns.filter((c) => this.isConfirmed(c.id)).length
  }

  get allReviewed(): boolean {
    return this.session != null && this.reviewedCount === this.session.concerns.length
  }

  isAcked(fileIndex: number): boolean {
    return this.draft.acknowledged_opaque.includes(fileIndex)
  }

  toggleAck(fileIndex: number): void {
    if (this.#locked) return
    const i = this.draft.acknowledged_opaque.indexOf(fileIndex)
    if (i >= 0) this.draft.acknowledged_opaque.splice(i, 1)
    else this.draft.acknowledged_opaque.push(fileIndex)
    this.scheduleSave()
  }

  /** Files that can't be judged from the rendered diff body alone (opaque
   * content, gitlink, mode change — see requiresAck) need an explicit ack
   * instead of a verdict-driven confirmation. */
  get allOpaqueAcked(): boolean {
    if (!this.session) return true
    return this.session.files.every((f, i) => !requiresAck(f) || this.isAcked(i))
  }

  /** Concern ids whose hunks include `ref` (used to render shared-hunk badges). */
  hunkOwners(ref: HunkRef): string[] {
    if (!this.session) return []
    return this.session.concerns
      .filter((c) => c.hunks.some((h) => h.file === ref.file && h.hunk === ref.hunk))
      .map((c) => c.id)
  }

  select(idx: number): void {
    if (!this.session) return
    if (idx < 0 || idx >= this.session.concerns.length) return
    this.selectedIdx = idx
    this.pendingCommentTarget = null
  }

  move(delta: 1 | -1): void {
    this.select(this.selectedIdx + delta)
  }

  editorBuffer(key: string): string {
    return this.editorBuffers.get(key) ?? ''
  }

  setEditorBuffer(key: string, value: string): void {
    this.editorBuffers.set(key, value)
  }

  clearEditorBuffer(key: string): void {
    this.editorBuffers.delete(key)
  }

  /** True if any editor buffer (general or inline) holds text beyond
   * whitespace — used by `hasUnsavedChanges` (beforeunload) and the submit
   * guard. Whitespace-only text (e.g. an editor opened and closed without
   * typing) must not count as unsaved. */
  #hasNonEmptyEditorBuffer(): boolean {
    for (const v of this.editorBuffers.values()) {
      if (v.trim()) return true
    }
    return false
  }

  /** True once the review has been finalized (submitted or aborted), or
   * while a submit/abort is actively in flight — mutations and saves are
   * inert in either case. */
  get #locked(): boolean {
    return (
      this.phase === 'submitted' ||
      this.phase === 'aborted' ||
      this.phase === 'timed_out' ||
      this.#actionLocked
    )
  }

  #ensureConcernDraft(id: string): ConcernDraft {
    let cd = this.draft.concerns[id]
    if (!cd) {
      cd = { verdict: null, comments: [] }
      this.draft.concerns[id] = cd
    }
    return cd
  }

  setVerdict(id: string, v: Verdict): void {
    if (this.#locked) return
    this.#ensureConcernDraft(id).verdict = v
    this.scheduleSave()
  }

  addComment(id: string, c: Comment): void {
    if (this.#locked) return
    this.#ensureConcernDraft(id).comments.push(c)
    this.scheduleSave()
  }

  removeComment(id: string, i: number): void {
    if (this.#locked) return
    this.#ensureConcernDraft(id).comments.splice(i, 1)
    this.scheduleSave()
  }

  addGeneralComment(body: string): void {
    if (this.#locked) return
    const trimmed = body.trim()
    if (!trimmed) return
    this.draft.general_comments.push(trimmed)
    this.scheduleSave()
  }

  removeGeneralComment(i: number): void {
    if (this.#locked) return
    this.draft.general_comments.splice(i, 1)
    this.scheduleSave()
  }

  async load(): Promise<void> {
    this.phase = 'loading'
    try {
      const session = await fetchSession()
      this.session = session
      // Older drafts predate acknowledged_opaque — default it so allOpaqueAcked
      // and toggleAck can assume the array always exists. The lenient PUT
      // /draft endpoint can also have persisted unknown/stale/duplicate
      // indices (e.g. from a stale client); normalize to the set of file
      // indices that actually require an ack so the UI can't get stuck
      // unrecoverable on bad saved state.
      const ackIdx = new Set(
        session.files.map((f, i) => (requiresAck(f) ? i : -1)).filter((i) => i >= 0),
      )
      const acked = [...new Set(session.draft.acknowledged_opaque ?? [])].filter((i) =>
        ackIdx.has(i),
      )
      this.draft = { ...session.draft, acknowledged_opaque: acked }
      this.#revision = session.draft_revision
      this.selectedIdx = 0
      this.phase = phaseForFinished(session.finished)
    } catch {
      this.phase = 'error'
    }
  }

  /** True while the latest draft may not be persisted on the server, OR
   * while an editor buffer holds text that was never committed via
   * `addComment`/`addGeneralComment` — that text lives only in this store
   * and would otherwise vanish silently on navigation. */
  get hasUnsavedChanges(): boolean {
    return (
      this.#saveTimer != null ||
      this.saveState === 'saving' ||
      this.saveState === 'error' ||
      this.#hasNonEmptyEditorBuffer()
    )
  }

  scheduleSave(): void {
    if (this.#locked || this.draftConflict) return
    if (this.#saveTimer != null) clearTimeout(this.#saveTimer)
    this.#saveTimer = setTimeout(() => {
      this.#saveTimer = null
      this.#queueSave()
    }, SAVE_DEBOUNCE_MS)
  }

  /** Runs `fn` only after everything already queued (a save PUT, a submit,
   * an abort) has settled, and leaves `#mutationChain` pointing at this call
   * so anything queued next waits its turn too. This is the single
   * serialization point: no two mutations against the server ever run
   * concurrently, and `fn` always sees `this.draft` / `this.#revision` as of
   * when it actually starts, not when it was scheduled. */
  #enqueueMutation<T>(fn: () => Promise<T>): Promise<T> {
    const result = this.#mutationChain.then(fn, fn)
    // The chain itself must never reject — a rejected #mutationChain would
    // permanently skip every `fn` chained after it.
    this.#mutationChain = result.then(
      () => undefined,
      () => undefined,
    )
    return result
  }

  /** Waits for any save currently running or queued to finish, so
   * `this.#revision` reflects the server's true latest state afterward.
   * Submit/abort call this before reading the draft/revision they send. */
  #flushSave(): Promise<void> {
    return this.#mutationChain
  }

  #queueSave(): void {
    // #enqueueMutation already updates #mutationChain in place; its return
    // value isn't needed here (unlike submit/abort, nothing inspects a
    // save's own result — #runSave applies it to state internally).
    this.#enqueueMutation(() => this.#runSave())
  }

  async #runSave(): Promise<void> {
    // Deliberately not `|| this.#locked`: a save already chained onto
    // #mutationChain before a submit/abort set the lock must still run to
    // completion — #flushSave() is waiting on exactly this, and skipping it
    // would leave #revision stale and reintroduce the P0-3 race under a
    // different name.
    if (this.draftConflict) return
    this.saveState = 'saving'
    // Fresh id for this save attempt. If the request throws (a lost
    // response — client timeout/network failure), it's retried once with
    // the SAME id and content immediately (no edit can land in between,
    // there's no `await` yielding to anything else): the server then
    // recognizes the retry as the same mutation instead of double-applying
    // it or 409ing on a revision the retry never learned advanced.
    const mutationId = crypto.randomUUID()
    // Snapshot the draft (and the revision it was taken against) ONCE,
    // before the first attempt, and reuse that identical payload for the
    // one-shot retry below. The retry can fire up to ~40s after the first
    // attempt started, and edits aren't locked during autosave — re-reading
    // `this.draft` for the retry could send a DIFFERENT payload under the
    // SAME mutation id. The server would then replay its "already applied"
    // response for the id, the client would mark it saved, and whatever
    // changed between the two attempts would be silently lost. Same id must
    // always mean same bytes.
    const draftSnapshot = structuredClone(this.draft)
    const revisionSnapshot = this.#revision
    try {
      let result: SaveDraftResult
      try {
        result = await saveDraft(draftSnapshot, revisionSnapshot, mutationId)
      } catch {
        result = await saveDraft(draftSnapshot, revisionSnapshot, mutationId)
      }
      if (!result.ok) {
        if (result.error === 'session finished') {
          // The session ended elsewhere; join the matching terminal state
          // (an abort must not render as "submitted") rather than
          // surfacing a save error.
          this.phase = phaseForFinished(result.finished ?? 'submitted')
          this.saveState = 'idle'
        } else {
          // Draft conflict: another tab saved a newer revision. Retrying
          // would overwrite it, so autosave stays off until a reload.
          this.draftConflict = true
          this.#cancelPendingSave()
          this.saveState = 'error'
        }
        return
      }
      this.#revision = result.revision
      this.saveState = 'saved'
      // A successful save is proof the session is alive: if a prior lost
      // response had left us showing the outcome-unknown banner, this
      // autosave landing is live evidence that wasn't true (or is no longer
      // true) — drop back to 'review' so the sticky banner clears instead
      // of requiring a manual reload.
      if (this.phase === 'outcome_unknown') this.phase = 'review'
    } catch (e) {
      if (!isAmbiguousFetchError(e)) {
        // A confirmed server response (saveDraft throws a plain Error for a
        // clean non-2xx/non-409 status, e.g. 413/422) — known, not
        // ambiguous. Recovery must not run; same handling as before
        // outcome-unknown recovery existed.
        this.saveState = 'error'
        return
      }
      // Both attempts (the original and its one same-id retry, above) lost
      // their response (AbortError/network error). Query the server rather
      // than assume failure — see #recoverFromLostResponse.
      const outcome = await this.#recoverFromLostResponse()
      if (outcome === 'unknown') {
        this.phase = 'outcome_unknown'
        this.saveState = 'error'
      } else if (outcome === 'review') {
        // Still reviewing server-side: the save never landed, but the
        // query itself is live evidence the session is alive — same
        // reasoning as the successful-save case above, so also clear a
        // standing outcome-unknown banner here. The next scheduleSave
        // (triggered by any draft change) retries the save itself.
        this.phase = 'review'
        this.saveState = 'error'
      } else {
        this.phase = outcome
        this.saveState = 'idle'
      }
    }
  }

  /** After a save/submit's fetch outright fails — the 40s client timeout's
   * AbortError, or a network error; never a clean HTTP error response
   * (409/422/413), which `saveDraft`/`submit` return as a value instead of
   * throwing — the outcome server-side is genuinely ambiguous: the mutation
   * may have gone through even though this tab never saw the response.
   * Rather than assume failure, a single `GET /session` tells us which:
   * `finished: 'submitted'`/`'aborted'`/`'timeout'` means the session
   * already ended server-side (the mutation is durable, possibly already
   * written to --out) and this tab should just join that terminal state;
   * still `Reviewing` means the mutation never landed and is safe to retry
   * (submit is idempotent per mutation id — Task 2.2). If the query itself
   * also fails, the outcome truly can't be determined from here — the
   * caller falls back to an "outcome unknown" banner rather than looping.
   */
  async #recoverFromLostResponse(): Promise<Phase | 'unknown'> {
    try {
      const session = await fetchSession()
      return phaseForFinished(session.finished)
    } catch {
      return 'unknown'
    }
  }

  #cancelPendingSave(): void {
    if (this.#saveTimer != null) {
      clearTimeout(this.#saveTimer)
      this.#saveTimer = null
    }
  }

  async submitReview(): Promise<void> {
    // A tab that already lost the draft-conflict race must not submit: the
    // server would refuse it anyway, and locally it would look like a
    // transient error rather than the standing "reload" condition.
    if (this.#locked || this.submitting || this.draftConflict) return
    // Unsent editor text (a comment box with text that was never committed
    // via addComment/addGeneralComment) would otherwise be silently dropped
    // by submitting — ask before discarding it. Buffers are intentionally
    // NOT auto-committed: the user may have left them mid-thought.
    if (this.#hasNonEmptyEditorBuffer()) {
      const proceed = globalThis.confirm(
        "You have unsaved comment text that hasn't been added. Discard and submit?",
      )
      if (!proceed) return
    }
    this.#cancelPendingSave()
    this.submitting = true
    // Locks editing/autosave for the duration of the submit, closing the
    // P0-3 race: without this, edits made while we're awaiting the flush
    // below could be scheduled as a save that runs concurrently with (or
    // right after) submit reads the draft, or could simply be left out of
    // what gets submitted.
    this.#actionLocked = true
    this.submitError = null
    try {
      // Wait for any in-flight/queued save (e.g. an autosave from just
      // before Submit was clicked) to land first, so the revision we submit
      // with is the server's true current one — not a stale snapshot that
      // the save-in-progress is about to invalidate.
      await this.#flushSave()
      // The flushed save may have surfaced a conflict, or ended the session
      // from elsewhere; either way there's nothing left for us to submit.
      // `outcome_unknown` is treated like `review` here: submit is
      // idempotent per mutation id (Task 2.2), so retrying from a lost-
      // response state the flushed save may have just left us in is safe —
      // without this the banner's "retry below" would be a silent no-op.
      if (this.draftConflict || (this.phase !== 'review' && this.phase !== 'outcome_unknown')) return
      const mutationId = crypto.randomUUID()
      const result = await this.#enqueueMutation(() => submit(this.draft, this.#revision, mutationId))
      if ('ok' in result) {
        this.phase = 'submitted'
        return
      }
      if (result.error === 'draft conflict') {
        // Same standing condition as a conflicting save: stop autosave and
        // show the reload banner instead of a transient submit error.
        this.draftConflict = true
        this.#cancelPendingSave()
        this.saveState = 'error'
        return
      }
      if (result.error === 'session finished') {
        this.phase = phaseForFinished(result.finished ?? 'submitted')
        return
      }
      const parts = [result.error]
      if (result.missing && result.missing.length > 0) parts.push(result.missing.join(', '))
      if (result.details && result.details.length > 0) parts.push(result.details.join(', '))
      this.submitError = parts.join(': ')
      // Unlock before rescheduling: scheduleSave is a no-op while locked.
      this.#actionLocked = false
      // Make sure the latest draft still gets persisted now that the
      // submit didn't go through.
      this.scheduleSave()
    } catch (e) {
      // submit() never throws for a clean HTTP error response — it always
      // parses the JSON body regardless of status — so this guard is
      // currently always true here. It's kept anyway so the two catch
      // paths (this one and #runSave's) stay uniform and future-proof
      // against submit() ever growing a status-based throw the way
      // saveDraft() has.
      if (!isAmbiguousFetchError(e)) {
        this.submitError = e instanceof Error ? e.message : 'Submit failed'
        this.#actionLocked = false
        this.scheduleSave()
      } else {
        const outcome = await this.#recoverFromLostResponse()
        if (outcome === 'unknown') {
          this.phase = 'outcome_unknown'
        } else if (outcome === 'review') {
          // The submit never landed server-side; safe to retry (submit is
          // idempotent per mutation id).
          this.submitError = 'Save/submit failed — you can retry.'
          this.#actionLocked = false
          this.scheduleSave()
        } else {
          // 'submitted' or 'aborted': the session already ended
          // server-side — join that terminal state instead of reporting
          // failure.
          this.phase = outcome
        }
      }
    } finally {
      this.submitting = false
      this.#actionLocked = false
    }
  }

  async abortReview(): Promise<void> {
    if (this.#locked || this.submitting) return
    this.#cancelPendingSave()
    this.submitting = true
    this.#actionLocked = true
    this.submitError = null
    try {
      // Same reasoning as submit: don't race an in-flight save — let it
      // land first so abort isn't chasing a PUT that's still on the wire.
      await this.#flushSave()
      // Same `outcome_unknown` allowance as submitReview above — abort is
      // safe to retry from a lost-response state too.
      if (this.phase !== 'review' && this.phase !== 'outcome_unknown') return
      await this.#enqueueMutation(() => abortSession())
      this.phase = 'aborted'
    } catch (e) {
      this.submitError = e instanceof Error ? e.message : 'Abort failed'
      this.#actionLocked = false
      // Same as submitReview: re-persist the draft after a failed abort.
      this.scheduleSave()
    } finally {
      this.submitting = false
      this.#actionLocked = false
    }
  }
}

export const rs = new ReviewState()
