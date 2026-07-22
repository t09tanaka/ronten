<script lang="ts">
  import { onMount } from 'svelte'
  import { GENERAL_BUFFER_KEY, rs } from './lib/state.svelte'
  import ConcernList from './lib/ConcernList.svelte'
  import DiffView from './lib/DiffView.svelte'
  import VerdictBar from './lib/VerdictBar.svelte'
  import { interpretKey } from './lib/keynav'
  import { revealControlChars } from './lib/invisibles'
  import { renderMarkdown } from './lib/markdown'
  import { focusGeneralComments } from './lib/scroll'
  import { formatCountdown, remainingMs } from './lib/countdown'
  import type { Verdict } from './lib/types'

  onMount(() => {
    void rs.load()
  })

  // Display-only countdown to session.deadline_at. Ticks once a second while
  // the review is active and a deadline exists; the effect's cleanup (return)
  // stops the interval both when the deadline disappears and when the phase
  // leaves 'review' (submitted/aborted/timed_out/etc.), so it never keeps
  // running past a terminal screen. This never ends the session itself —
  // when it reaches zero the server ends the session on its own and the next
  // GET/action reflects that; no client-side auto-abort here.
  let now = $state(Date.now())
  $effect(() => {
    if (rs.phase !== 'review' || !rs.session?.deadline_at) return
    const id = setInterval(() => {
      now = Date.now()
    }, 1000)
    return () => clearInterval(id)
  })
  const remainingCountdown = $derived(
    rs.phase === 'review' && rs.session?.deadline_at
      ? formatCountdown(remainingMs(rs.session.deadline_at, now))
      : null,
  )

  let showSubmitConfirm = $state(false)
  let showAbortConfirm = $state(false)
  // Backed by the central store (not local $state) so in-progress text
  // survives navigation within the page — see Task 2.5 / P1-5.
  const generalCommentText = $derived(rs.editorBuffer(GENERAL_BUFFER_KEY))
  let warningsDismissed = $state(false)

  // Native <dialog> elements driven by the show* flags. showModal()/close()
  // are imperative, so an effect keeps the DOM in sync with the state, and
  // the dialogs' close events sync the state back (e.g. native Escape).
  let submitDialog = $state<HTMLDialogElement | null>(null)
  let abortDialog = $state<HTMLDialogElement | null>(null)

  $effect(() => {
    const d = submitDialog
    if (!d) return
    if (showSubmitConfirm && !d.open) d.showModal()
    else if (!showSubmitConfirm && d.open) d.close()
  })

  $effect(() => {
    const d = abortDialog
    if (!d) return
    if (showAbortConfirm && !d.open) d.showModal()
    else if (!showAbortConfirm && d.open) d.close()
  })

  const VERDICT_LABELS: Record<Verdict, string> = {
    approve: 'Approve',
    'request-changes': 'Request changes',
  }

  function verdictLabel(v: Verdict | null | undefined): string {
    return v ? VERDICT_LABELS[v] : 'No verdict'
  }

  function openSubmitConfirm(): void {
    rs.submitError = null
    showSubmitConfirm = true
  }

  function openAbortConfirm(): void {
    rs.submitError = null
    showAbortConfirm = true
  }

  // Best-effort auto-close once the review is finished in THIS tab (not on
  // reloading an already-finished session). Browsers only honor
  // window.close() for tabs they consider script-closable — typically ones
  // with a single history entry, like the tab the CLI opened. When the
  // browser refuses, the terminal-state message stays as the fallback.
  function closeTabSoon(): void {
    setTimeout(() => window.close(), 800)
  }

  async function confirmSubmit(): Promise<void> {
    await rs.submitReview()
    if (rs.phase === 'submitted') {
      showSubmitConfirm = false
      closeTabSoon()
    } else if (rs.phase === 'outcome_unknown') {
      // Close the confirm dialog so the outcome-unknown banner (rendered
      // behind it) becomes visible instead of being hidden by the dialog's
      // top-layer backdrop.
      showSubmitConfirm = false
    }
  }

  async function confirmAbort(): Promise<void> {
    await rs.abortReview()
    if (rs.phase === 'aborted') {
      showAbortConfirm = false
      closeTabSoon()
    }
  }

  function addGeneralComment(): void {
    rs.addGeneralComment(generalCommentText)
    rs.clearEditorBuffer(GENERAL_BUFFER_KEY)
  }

  let draftCopyState = $state<'idle' | 'copied' | 'error'>('idle')

  // A genuine cross-tab draft conflict means this tab's edits can no longer
  // be saved and the only way forward is a reload — copying the local draft
  // out first is the one way to avoid silently losing that work.
  async function copyDraftJson(): Promise<void> {
    try {
      await navigator.clipboard.writeText(JSON.stringify(rs.draft, null, 2))
      draftCopyState = 'copied'
    } catch {
      draftCopyState = 'error'
    }
    setTimeout(() => {
      draftCopyState = 'idle'
    }, 2000)
  }

  function scrollSelectedIntoView(): void {
    document.querySelector(`[data-idx="${rs.selectedIdx}"]`)?.scrollIntoView({ block: 'nearest' })
  }

  // Global keyboard shortcuts for the review flow. Escape is handled here
  // directly (not via interpretKey — that contract only maps binding keys)
  // so it can close confirm panels / the inline comment editor regardless
  // of focus. All other keys go through interpretKey, which already
  // returns null while typing in an input/textarea/select/contenteditable.
  function handleKeydown(e: KeyboardEvent): void {
    if (rs.phase !== 'review') return

    const target = e.target as HTMLElement | null

    if (e.key === 'Escape') {
      // While a confirm dialog is open the native <dialog> handles Escape
      // itself (cancel event) — don't preventDefault or double-handle.
      if (showSubmitConfirm || showAbortConfirm) return
      e.preventDefault()
      if (rs.pendingCommentTarget) {
        rs.pendingCommentTarget = null
      }
      return
    }

    // A focused button keeps its native key handling: Enter must activate
    // the button itself (e.g. Cancel in the submit confirm — intercepting
    // it as confirm-submit would turn "cancel" into an accidental,
    // irreversible submission). j/k/a/x/c fall through here too; the
    // global shortcuts remain active for body/other targets.
    if (target?.tagName === 'BUTTON') return

    const confirmOpen = showSubmitConfirm || showAbortConfirm
    const action = interpretKey({
      key: e.key,
      ctrlKey: e.ctrlKey,
      metaKey: e.metaKey,
      altKey: e.altKey,
      isComposing: e.isComposing,
      targetTag: target?.tagName ?? '',
      targetEditable: target?.isContentEditable ?? false,
    })
    if (!action) return

    switch (action.type) {
      case 'move':
        if (confirmOpen) return
        e.preventDefault()
        rs.move(action.delta)
        scrollSelectedIntoView()
        break
      case 'verdict':
        if (confirmOpen) return
        e.preventDefault()
        if (rs.selected) {
          rs.setVerdict(rs.selected.id, action.verdict)
          // Request-changes means "I have something to write" — bring the
          // comment box into view and focus it, like the `i` shortcut.
          if (action.verdict !== 'approve') focusGeneralComments()
        }
        break
      case 'focus-comment':
        if (confirmOpen) return
        e.preventDefault()
        document.getElementById('general-comment-textarea')?.focus()
        break
      case 'confirm-submit':
        e.preventDefault()
        if (showSubmitConfirm) {
          void confirmSubmit()
        } else if (!confirmOpen && rs.allReviewed && rs.allOpaqueAcked) {
          openSubmitConfirm()
        }
        break
    }
  }

  // Warn before leaving while draft changes may not be persisted yet
  // (debounced save pending, save in flight, or last save failed).
  // outcome_unknown counts too — a lost save response leaves saveState
  // 'error' there just like an ordinary retryable failure, and the user
  // must not be able to navigate away from unsaved work while confused
  // about whether their last action even landed.
  function handleBeforeUnload(e: BeforeUnloadEvent): void {
    if (rs.phase !== 'review' && rs.phase !== 'outcome_unknown') return
    if (!rs.hasUnsavedChanges) return
    e.preventDefault()
  }

  const maxCommentChars = $derived(rs.limits?.max_comment_chars)

  const submitTitle = $derived(
    !rs.allReviewed
      ? `Every concern needs a verdict — request changes also needs a comment (${(rs.session?.concerns.length ?? 0) - rs.reviewedCount} remaining)`
      : !rs.allOpaqueAcked
        ? 'Acknowledge all flagged changes (undisplayed contents, mode or submodule changes) to submit'
        : 'Submit review',
  )
</script>

<svelte:window onkeydown={handleKeydown} onbeforeunload={handleBeforeUnload} />

{#if rs.phase === 'loading'}
  <div class="center-message">Loading…</div>
{:else if rs.phase === 'error'}
  <div class="center-message">Session not found or already finished.</div>
{:else if rs.phase === 'submitted'}
  <div class="center-message">Review submitted. You can close this tab.</div>
{:else if rs.phase === 'aborted'}
  <div class="center-message">Review aborted. You can close this tab.</div>
{:else if rs.phase === 'timed_out'}
  <div class="center-message">Review timed out — the session ended before you submitted.</div>
{:else if rs.session}
  <div class="app">
    <header class="topbar">
      <div class="topbar-title">
        <span class="seal" aria-hidden="true">論</span>
        <div class="topbar-text">
          <h1>{revealControlChars(rs.session.title)}</h1>
          {#if rs.session.summary}
            <p class="summary">{revealControlChars(rs.session.summary)}</p>
          {/if}
        </div>
      </div>
      <div class="topbar-status">
        {#if remainingCountdown}
          <span class="countdown-indicator" aria-live="polite" title="Time remaining before this session times out"
            >{remainingCountdown}</span
          >
        {/if}
        <span
          class="save-indicator"
          class:save-indicator-error={rs.saveState === 'error'}
          aria-live="polite"
        >
          {#if rs.saveState === 'saving'}Saving…{:else if rs.saveState === 'saved'}Saved{:else if rs.saveState === 'error'}Save failed{/if}
        </span>
      </div>
    </header>
    {#if rs.draftConflict}
      <div class="conflict-banner" role="alert">
        <span>
          This review was edited in another tab. Changes made here are no longer being saved —
          reload the page to continue.
        </span>
        <button type="button" class="btn-ghost conflict-banner-copy" onclick={copyDraftJson}>
          {draftCopyState === 'copied'
            ? 'Copied'
            : draftCopyState === 'error'
              ? 'Copy failed'
              : 'Copy draft JSON'}
        </button>
      </div>
    {/if}
    {#if rs.phase === 'outcome_unknown'}
      <div class="conflict-banner" role="alert">
        <span>
          Result unknown — we lost the connection and couldn't confirm whether your last action
          went through. Check the CLI output or --out file, or retry below.
        </span>
        <button type="button" class="btn-ghost conflict-banner-copy" onclick={copyDraftJson}>
          {draftCopyState === 'copied'
            ? 'Copied'
            : draftCopyState === 'error'
              ? 'Copy failed'
              : 'Copy draft JSON'}
        </button>
      </div>
    {/if}
    {#if rs.session.warnings.length > 0 && !warningsDismissed}
      <div class="warnings-banner">
        <div class="warnings-banner-header">
          <span class="warnings-banner-title">Mapping warnings</span>
          <button
            type="button"
            class="warnings-banner-dismiss"
            aria-label="Dismiss warnings"
            onclick={() => (warningsDismissed = true)}>×</button
          >
        </div>
        <ul class="warnings-banner-list">
          {#each rs.session.warnings as warning, i (i)}
            <li>
              {#if warning.path}<span class="warning-path">{revealControlChars(warning.path)}:</span
                >{/if}
              {revealControlChars(warning.message)}
            </li>
          {/each}
        </ul>
      </div>
    {/if}
    <div class="body">
      <aside class="left-pane">
        <div class="concern-scroll">
          <ConcernList />
        </div>
        <!-- Actions live at the end of the concern list — the natural
             stopping point of the review flow — not in the topbar where
             they get overlooked. -->
        <div class="sidebar-footer">
          <span class="reviewed-counter"
            >{rs.reviewedCount}/{rs.session.concerns.length} reviewed</span
          >
          <button
            type="button"
            class="btn-primary"
            disabled={!rs.allReviewed || !rs.allOpaqueAcked || rs.draftConflict}
            title={submitTitle}
            onclick={openSubmitConfirm}>Submit review</button
          >
          <button type="button" class="btn-ghost" onclick={openAbortConfirm}>Abort review</button>
        </div>
      </aside>
      <main class="main-pane">
        {#if rs.selected}
          {@const selected = rs.selected}
          {@const selectedComments = rs.draft.concerns[selected.id]?.comments ?? []}
          <div class="concern-header">
            <h2>{revealControlChars(selected.title)}</h2>
            {#if selected.risk}
              <span class="risk-badge risk-{selected.risk}">{selected.risk}</span>
            {/if}
            {#if selected.unmapped}
              <span class="unmapped-tag">unmapped</span>
            {/if}
          </div>
          <VerdictBar concernId={selected.id} />
          {#if selectedComments.length > 0}
            <ul class="concern-comment-list">
              {#each selectedComments as comment, i (i)}
                <li>
                  <span class="comment-loc">{revealControlChars(comment.path)}:{comment.line}</span>
                  {comment.body}
                </li>
              {/each}
            </ul>
          {/if}
          {#if selected.description}
            <div class="concern-description">
              {@html renderMarkdown(selected.description)}
            </div>
          {/if}
          <DiffView />
        {/if}
        <section class="general-comments">
          <h3>General comments</h3>
          <textarea
            id="general-comment-textarea"
            bind:value={() => generalCommentText, (v) => rs.setEditorBuffer(GENERAL_BUFFER_KEY, v)}
            placeholder="Add a general comment…"
            rows="3"
            maxlength={maxCommentChars}
          ></textarea>
          {#if maxCommentChars != null && generalCommentText.length > maxCommentChars * 0.9}
            <span class="char-count">{generalCommentText.length}/{maxCommentChars}</span>
          {/if}
          <button
            type="button"
            class="btn-outline"
            onclick={addGeneralComment}
            disabled={!generalCommentText.trim()}>Add comment</button
          >
          {#if rs.draft.general_comments.length > 0}
            <ul class="general-comment-list">
              {#each rs.draft.general_comments as comment, i (i)}
                <li>
                  <span class="comment-body">{comment}</span>
                  <button
                    type="button"
                    class="comment-delete"
                    aria-label="Delete comment"
                    onclick={() => rs.removeGeneralComment(i)}>×</button
                  >
                </li>
              {/each}
            </ul>
          {/if}
        </section>
      </main>
    </div>
    <footer class="shortcut-hint">
      <kbd>j</kbd>/<kbd>k</kbd> select · <kbd>a</kbd> approve · <kbd>x</kbd> request changes ·
      <kbd>Enter</kbd> submit · <kbd>i</kbd> comment box · <kbd>Esc</kbd> close
    </footer>
  </div>

  <dialog
    bind:this={submitDialog}
    class="modal-panel"
    aria-labelledby="submit-dialog-title"
    oncancel={() => (showSubmitConfirm = false)}
    onclose={() => (showSubmitConfirm = false)}
  >
    <h2 id="submit-dialog-title">Submit review</h2>
    <ul class="verdict-summary">
      {#each rs.session.concerns as c (c.id)}
        <li>
          <span class="vs-title">{revealControlChars(c.title)}</span>
          <span class="vs-verdict vs-{rs.draft.concerns[c.id]?.verdict ?? 'none'}"
            >{verdictLabel(rs.draft.concerns[c.id]?.verdict)}</span
          >
        </li>
      {/each}
    </ul>
    {#if rs.submitError}
      <p class="modal-error">{rs.submitError}</p>
    {/if}
    <div class="modal-actions">
      <button type="button" class="btn-primary" onclick={confirmSubmit} disabled={rs.submitting}
        >{rs.submitting ? 'Submitting…' : 'Submit review'}</button
      >
      <button
        type="button"
        class="btn-ghost"
        onclick={() => (showSubmitConfirm = false)}
        disabled={rs.submitting}>Cancel</button
      >
    </div>
  </dialog>

  <dialog
    bind:this={abortDialog}
    class="modal-panel"
    aria-label="Abort review"
    oncancel={() => (showAbortConfirm = false)}
    onclose={() => (showAbortConfirm = false)}
  >
    <p>Abort review? The agent will receive exit code 2 and no result.</p>
    {#if rs.submitError}
      <p class="modal-error">{rs.submitError}</p>
    {/if}
    <div class="modal-actions">
      <button type="button" class="btn-danger" onclick={confirmAbort} disabled={rs.submitting}
        >{rs.submitting ? 'Aborting…' : 'Abort review'}</button
      >
      <button
        type="button"
        class="btn-ghost"
        onclick={() => (showAbortConfirm = false)}
        disabled={rs.submitting}>Cancel</button
      >
    </div>
  </dialog>
{/if}

<style>
  .app {
    display: flex;
    flex-direction: column;
    height: 100vh;
  }

  .topbar {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 16px;
    padding: 10px 16px;
    border-bottom: 1px solid var(--c-rule);
    background: var(--c-panel);
    flex-wrap: wrap;
  }

  .topbar-title {
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 0;
  }

  .seal {
    flex-shrink: 0;
    width: 28px;
    height: 28px;
    border-radius: 2px;
    background: var(--c-shu);
    color: var(--c-paper);
    font-family: var(--font-display);
    font-size: 16px;
    font-weight: 600;
    display: grid;
    place-items: center;
    user-select: none;
  }

  .topbar-text h1 {
    margin: 0;
    font-family: var(--font-display);
    font-size: 17px;
    font-weight: 600;
    letter-spacing: 0.01em;
    /* Trojan Source defense: isolate agent-supplied text so bidi control
       characters in it can't reorder neighboring elements (their codepoints
       are also revealed as ⟨U+XXXX⟩ tokens by revealControlChars). */
    unicode-bidi: isolate;
  }

  .topbar-text .summary {
    margin: 2px 0 0;
    font-size: 12px;
    color: var(--c-ink-2);
    max-width: 60ch;
    unicode-bidi: isolate;
  }

  .reviewed-counter {
    font-size: 13px;
    color: var(--c-ink-2);
    font-variant-numeric: tabular-nums;
  }

  .topbar-status {
    flex-shrink: 0;
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .countdown-indicator {
    font-size: 12px;
    font-variant-numeric: tabular-nums;
    color: var(--c-ink-2);
  }

  .save-indicator {
    flex-shrink: 0;
    font-size: 12px;
    color: var(--c-ink-3);
  }

  .save-indicator-error {
    color: var(--c-shu);
  }

  .btn-primary {
    padding: 6px 14px;
    border: 1px solid var(--c-shu);
    border-radius: 3px;
    background: var(--c-shu);
    color: #fff;
    font-size: 13px;
    font-family: inherit;
    cursor: pointer;
  }

  .btn-primary:hover:not(:disabled) {
    background: #a83619;
    border-color: #a83619;
  }

  .btn-primary:disabled {
    background: #e9e6dd;
    border-color: #e9e6dd;
    color: var(--c-ink-3);
    cursor: not-allowed;
  }

  .btn-ghost {
    padding: 6px 14px;
    border: 1px solid var(--c-rule);
    border-radius: 3px;
    background: transparent;
    color: var(--c-ink-2);
    font-size: 13px;
    font-family: inherit;
    cursor: pointer;
  }

  .btn-ghost:hover:not(:disabled) {
    background: var(--c-hover-wash);
    color: var(--c-ink);
  }

  .btn-ghost:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .btn-outline {
    padding: 6px 14px;
    border: 1px solid var(--c-ai);
    border-radius: 3px;
    background: var(--c-paper);
    color: var(--c-ai);
    font-size: 13px;
    font-family: inherit;
    cursor: pointer;
  }

  .btn-outline:hover:not(:disabled) {
    background: var(--c-ai-tint);
  }

  .btn-outline:disabled {
    border-color: var(--c-rule);
    color: var(--c-ink-3);
    cursor: not-allowed;
  }

  /* Destructive confirm (abort): shu outline, never filled — the filled
     shu treatment is reserved for Submit alone. */
  .btn-danger {
    padding: 6px 14px;
    border: 1px solid var(--c-shu);
    border-radius: 3px;
    background: var(--c-paper);
    color: var(--c-shu);
    font-size: 13px;
    font-family: inherit;
    cursor: pointer;
  }

  .btn-danger:hover:not(:disabled) {
    background: var(--c-shu-tint);
  }

  .btn-danger:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  /* Persistent: unlike the warnings banner there is no dismiss — the only
     way out is a reload, so the message must stay visible. */
  .conflict-banner {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 8px 16px;
    background: var(--c-shu-tint);
    border-bottom: 1px solid var(--c-shu-tint-2);
    color: var(--c-shu);
    font-size: 13px;
  }

  .conflict-banner-copy {
    flex-shrink: 0;
    border-color: var(--c-shu);
    color: var(--c-shu);
  }

  .warnings-banner {
    padding: 8px 16px;
    background: var(--c-odo-tint);
    border-bottom: 1px solid #ead9ab;
    color: var(--c-odo);
  }

  .warnings-banner-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
  }

  .warnings-banner-title {
    font-size: 12px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .warnings-banner-dismiss {
    flex-shrink: 0;
    border: none;
    background: none;
    color: var(--c-odo);
    cursor: pointer;
    font-size: 14px;
    line-height: 1;
    padding: 0 2px;
  }

  .warnings-banner-dismiss:hover {
    color: #6b4900;
  }

  .warnings-banner-list {
    list-style: none;
    margin: 4px 0 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .warnings-banner-list li {
    font-size: 12px;
    line-height: 1.4;
    /* Trojan Source defense: warnings may quote agent-supplied paths. */
    unicode-bidi: isolate;
  }

  .warning-path {
    font-family: var(--font-mono);
    font-size: 11.5px;
    /* Trojan Source defense: the path itself is agent-supplied — pin
       display order to logical order. */
    unicode-bidi: isolate;
    direction: ltr;
  }

  .body {
    flex: 1;
    display: flex;
    min-height: 0;
  }

  .left-pane {
    width: 280px;
    flex: 0 0 280px;
    display: flex;
    flex-direction: column;
    border-right: 1px solid var(--c-rule);
    background: var(--c-panel);
    min-height: 0;
  }

  .concern-scroll {
    flex: 1;
    overflow-y: auto;
    min-height: 0;
  }

  .sidebar-footer {
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 12px 14px;
    border-top: 1px solid var(--c-rule);
  }

  .sidebar-footer .reviewed-counter {
    text-align: center;
  }

  .sidebar-footer .btn-primary,
  .sidebar-footer .btn-ghost {
    width: 100%;
  }

  .main-pane {
    flex: 1;
    overflow-y: auto;
    padding: 16px 20px 24px;
    min-width: 0;
  }

  .concern-header {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 8px;
  }

  .concern-header h2 {
    margin: 0;
    font-family: var(--font-display);
    font-size: 20px;
    font-weight: 600;
    unicode-bidi: isolate;
  }

  .concern-description {
    font-size: 14px;
    line-height: 1.55;
    color: var(--c-ink);
    margin-bottom: 16px;
    max-width: 80ch;
    unicode-bidi: isolate;
  }

  .concern-description :global(p) {
    margin: 0 0 10px;
  }

  .concern-description :global(ul) {
    margin: 0 0 10px 20px;
    padding: 0;
  }

  .concern-description :global(code) {
    background: var(--c-neutral-tint);
    padding: 1px 4px;
    border-radius: 3px;
    font-family: var(--font-mono);
    font-size: 13px;
  }

  .concern-description :global(pre) {
    background: var(--c-panel);
    border: 1px solid var(--c-rule);
    padding: 10px;
    border-radius: 4px;
    overflow-x: auto;
  }

  .concern-description :global(pre code) {
    background: none;
    padding: 0;
    border: none;
  }

  .concern-comment-list {
    list-style: none;
    margin: 0 0 14px;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .concern-comment-list li {
    font-size: 13px;
    color: var(--c-ink);
    background: var(--c-ai-tint);
    border: 1px solid var(--c-ai-border);
    border-radius: 4px;
    padding: 6px 10px;
  }

  .comment-loc {
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--c-ink-2);
    margin-right: 6px;
    /* Trojan Source defense: this echoes the file path the comment anchors
       to (agent-supplied) — pin display order to logical order. */
    unicode-bidi: isolate;
    direction: ltr;
  }

  .general-comments {
    margin-top: 32px;
    padding-top: 16px;
    border-top: 1px solid var(--c-rule);
    max-width: 80ch;
  }

  .general-comments h3 {
    margin: 0 0 10px;
    font-family: var(--font-display);
    font-size: 15px;
    font-weight: 600;
  }

  .general-comments textarea {
    width: 100%;
    box-sizing: border-box;
    font-family: inherit;
    font-size: 13px;
    padding: 8px;
    border: 1px solid var(--c-control-border);
    border-radius: 4px;
    background: var(--c-paper);
    color: var(--c-ink);
    resize: vertical;
  }

  .general-comments > .btn-outline {
    margin-top: 8px;
  }

  .char-count {
    display: block;
    margin-top: 2px;
    font-size: 11px;
    color: var(--c-ink-3);
    text-align: right;
    font-variant-numeric: tabular-nums;
  }

  .general-comment-list {
    list-style: none;
    margin: 14px 0 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .general-comment-list li {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 10px;
    font-size: 13px;
    color: var(--c-ink);
    background: var(--c-panel);
    border: 1px solid var(--c-rule);
    border-radius: 4px;
    padding: 8px 12px;
    white-space: pre-wrap;
  }

  .comment-delete {
    flex-shrink: 0;
    border: none;
    background: none;
    color: var(--c-ink-2);
    cursor: pointer;
    font-size: 14px;
    line-height: 1;
    padding: 0 2px;
  }

  .comment-delete:hover {
    color: var(--c-shu);
  }

  /* Native <dialog>: showModal() centers it in the top layer and provides
     the focus trap + focus restore; only the panel look is ours. */
  .modal-panel {
    border: none;
    background: var(--c-paper);
    border-radius: 6px;
    padding: 20px 24px;
    max-width: 440px;
    width: 90%;
    max-height: 80vh;
    overflow-y: auto;
    box-shadow: 0 8px 30px rgba(33, 31, 28, 0.18);
  }

  .modal-panel::backdrop {
    background: rgba(33, 31, 28, 0.45);
  }

  .modal-panel h2 {
    margin: 0 0 12px;
    font-family: var(--font-display);
    font-size: 17px;
    font-weight: 600;
  }

  .modal-panel p {
    margin: 0 0 12px;
    font-size: 14px;
    color: var(--c-ink);
  }

  .verdict-summary {
    list-style: none;
    margin: 0 0 14px;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
    max-height: 260px;
    overflow-y: auto;
  }

  .verdict-summary li {
    display: flex;
    justify-content: space-between;
    gap: 10px;
    font-size: 13px;
    padding: 4px 0;
    border-bottom: 1px solid var(--c-rule);
  }

  .vs-title {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    unicode-bidi: isolate;
  }

  .vs-verdict {
    flex-shrink: 0;
    color: var(--c-ink-2);
  }

  .vs-approve {
    color: var(--c-matsuba);
  }

  .vs-request-changes {
    color: var(--c-shu);
  }


  .modal-error {
    font-size: 13px;
    color: var(--c-shu);
    background: var(--c-shu-tint);
    border: 1px solid var(--c-shu-tint-2);
    border-radius: 4px;
    padding: 8px 10px;
    margin: 0 0 12px;
  }

  .modal-actions {
    display: flex;
    gap: 10px;
    justify-content: flex-end;
  }

  /* A normal layout row (not an overlay), so it can never cover content
     regardless of wrapping, zoom, or font size. */
  .shortcut-hint {
    flex-shrink: 0;
    padding: 4px 16px;
    font-size: 11px;
    color: var(--c-ink-2);
    background: var(--c-paper);
    border-top: 1px solid var(--c-rule);
    text-align: center;
  }

  .shortcut-hint kbd {
    font-family: var(--font-mono);
    font-size: 10px;
    padding: 1px 4px;
    border: 1px solid var(--c-rule);
    border-bottom-width: 2px;
    border-radius: 3px;
    background: #fff;
    color: var(--c-ink-2);
  }
</style>
