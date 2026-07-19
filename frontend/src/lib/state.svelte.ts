// Single shared review store (Svelte 5 runes). See task-11-brief.md for the
// contract this class implements.

import { fetchSession, saveDraft } from './api'
import type { Comment, ConcernDraft, ConcernView, Draft, HunkRef, Session, Verdict } from './types'

const SAVE_DEBOUNCE_MS = 500

export type Phase = 'loading' | 'review' | 'submitted' | 'aborted' | 'error'

class ReviewState {
  session = $state<Session | null>(null)
  draft = $state<Draft>({ concerns: {}, general_comments: [] })
  selectedIdx = $state(0)
  phase = $state<Phase>('loading')

  #saveTimer: ReturnType<typeof setTimeout> | null = null

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
  }

  move(delta: 1 | -1): void {
    this.select(this.selectedIdx + delta)
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
    this.#ensureConcernDraft(id).verdict = v
    this.scheduleSave()
  }

  addComment(id: string, c: Comment): void {
    this.#ensureConcernDraft(id).comments.push(c)
    this.scheduleSave()
  }

  removeComment(id: string, i: number): void {
    this.#ensureConcernDraft(id).comments.splice(i, 1)
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

  scheduleSave(): void {
    if (this.#saveTimer != null) clearTimeout(this.#saveTimer)
    this.#saveTimer = setTimeout(() => {
      this.#saveTimer = null
      void saveDraft(this.draft)
    }, SAVE_DEBOUNCE_MS)
  }
}

export const rs = new ReviewState()
