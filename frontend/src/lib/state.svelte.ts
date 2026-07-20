// Single shared review store (Svelte 5 runes).

import { abortSession, fetchSession, saveDraft, submit } from './api'
import type {
  Comment,
  ConcernDraft,
  ConcernView,
  Draft,
  HunkRef,
  Session,
  Side,
  Verdict,
} from './types'

const SAVE_DEBOUNCE_MS = 500

export type Phase = 'loading' | 'review' | 'submitted' | 'aborted' | 'error'
export type SaveState = 'idle' | 'saving' | 'saved' | 'error'

export interface CommentTarget {
  path: string
  side: Side
  line: number
}

class ReviewState {
  session = $state<Session | null>(null)
  draft = $state<Draft>({ concerns: {}, general_comments: [] })
  selectedIdx = $state(0)
  phase = $state<Phase>('loading')
  submitting = $state(false)
  submitError = $state<string | null>(null)
  pendingCommentTarget = $state<CommentTarget | null>(null)
  saveState = $state<SaveState>('idle')

  #saveTimer: ReturnType<typeof setTimeout> | null = null
  #saveInFlight = false
  #saveQueued = false

  get selected(): ConcernView | null {
    return this.session?.concerns[this.selectedIdx] ?? null
  }

  get reviewedCount(): number {
    if (!this.session) return 0
    return this.session.concerns.filter((c) => this.draft.concerns[c.id]?.verdict != null).length
  }

  get allReviewed(): boolean {
    return this.session != null && this.reviewedCount === this.session.concerns.length
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

  /** True once the review has been finalized (submitted or aborted) — mutations and saves are inert past this point. */
  get #locked(): boolean {
    return this.phase === 'submitted' || this.phase === 'aborted'
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
      this.draft = session.draft
      this.selectedIdx = 0
      this.phase = session.submitted ? 'submitted' : 'review'
    } catch {
      this.phase = 'error'
    }
  }

  /** True while the latest draft may not be persisted on the server. */
  get hasUnsavedChanges(): boolean {
    return this.#saveTimer != null || this.saveState === 'saving' || this.saveState === 'error'
  }

  scheduleSave(): void {
    if (this.#locked) return
    if (this.#saveTimer != null) clearTimeout(this.#saveTimer)
    this.#saveTimer = setTimeout(() => {
      this.#saveTimer = null
      void this.#runSave()
    }, SAVE_DEBOUNCE_MS)
  }

  // Serializes draft PUTs: at most one request in flight, and a save
  // requested while one is running re-reads the (then-current) draft after
  // it finishes. This prevents an older payload from racing past a newer
  // one and overwriting it on the server.
  async #runSave(): Promise<void> {
    if (this.#saveInFlight) {
      this.#saveQueued = true
      return
    }
    this.#saveInFlight = true
    this.saveState = 'saving'
    try {
      do {
        this.#saveQueued = false
        await saveDraft(this.draft)
      } while (this.#saveQueued)
      this.saveState = 'saved'
    } catch {
      // The next scheduleSave (triggered by any draft change) retries.
      this.saveState = 'error'
    } finally {
      this.#saveInFlight = false
    }
  }

  #cancelPendingSave(): void {
    if (this.#saveTimer != null) {
      clearTimeout(this.#saveTimer)
      this.#saveTimer = null
    }
  }

  async submitReview(): Promise<void> {
    if (this.#locked || this.submitting) return
    this.#cancelPendingSave()
    this.submitting = true
    this.submitError = null
    try {
      const result = await submit(this.draft)
      if ('ok' in result) {
        this.phase = 'submitted'
        return
      }
      const parts = [result.error]
      if (result.missing && result.missing.length > 0) parts.push(result.missing.join(', '))
      if (result.details && result.details.length > 0) parts.push(result.details.join(', '))
      this.submitError = parts.join(': ')
      // The pending save was cancelled above; make sure the latest draft
      // still gets persisted now that the submit didn't go through.
      this.scheduleSave()
    } catch (e) {
      this.submitError = e instanceof Error ? e.message : 'Submit failed'
      this.scheduleSave()
    } finally {
      this.submitting = false
    }
  }

  async abortReview(): Promise<void> {
    if (this.#locked || this.submitting) return
    this.#cancelPendingSave()
    this.submitting = true
    this.submitError = null
    try {
      await abortSession()
      this.phase = 'aborted'
    } catch (e) {
      this.submitError = e instanceof Error ? e.message : 'Abort failed'
      // Same as submitReview: re-persist the draft after a failed abort.
      this.scheduleSave()
    } finally {
      this.submitting = false
    }
  }
}

export const rs = new ReviewState()
