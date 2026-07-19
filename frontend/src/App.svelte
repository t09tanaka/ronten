<script lang="ts">
  import { onMount } from 'svelte'
  import { rs } from './lib/state.svelte'
  import ConcernList from './lib/ConcernList.svelte'
  import DiffView from './lib/DiffView.svelte'
  import VerdictBar from './lib/VerdictBar.svelte'
  import { interpretKey } from './lib/keynav'
  import type { Verdict } from './lib/types'

  onMount(() => {
    void rs.load()
  })

  let showSubmitConfirm = $state(false)
  let showAbortConfirm = $state(false)
  let generalCommentText = $state('')
  let warningsDismissed = $state(false)

  const VERDICT_LABELS: Record<Verdict, string> = {
    approve: 'Approve',
    'request-changes': 'Request changes',
    comment: 'Comment',
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

  async function confirmSubmit(): Promise<void> {
    await rs.submitReview()
    if (rs.phase === 'submitted') showSubmitConfirm = false
  }

  async function confirmAbort(): Promise<void> {
    await rs.abortReview()
    if (rs.phase === 'aborted') showAbortConfirm = false
  }

  function addGeneralComment(): void {
    rs.addGeneralComment(generalCommentText)
    generalCommentText = ''
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
      e.preventDefault()
      if (showSubmitConfirm) {
        showSubmitConfirm = false
      } else if (showAbortConfirm) {
        showAbortConfirm = false
      } else if (rs.pendingCommentTarget) {
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
    const action = interpretKey(e.key, target?.tagName ?? '', target?.isContentEditable ?? false)
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
        if (rs.selected) rs.setVerdict(rs.selected.id, action.verdict)
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
        } else if (!confirmOpen && rs.allReviewed) {
          openSubmitConfirm()
        }
        break
    }
  }

  // Tiny hand-rolled markdown converter for concern descriptions: paragraphs,
  // `- ` bullet lists, `code` spans, and fenced ``` code blocks. No external
  // markdown dependency. HTML is escaped first since descriptions come from
  // agent-supplied JSON.
  function escapeHtml(s: string): string {
    return s
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;')
      .replace(/'/g, '&#39;')
  }

  function renderInline(s: string): string {
    return s.replace(/`([^`]+)`/g, '<code>$1</code>')
  }

  function renderMarkdown(src: string): string {
    const lines = escapeHtml(src).split('\n')
    const out: string[] = []
    let paragraph: string[] = []
    let list: string[] = []

    function flushParagraph(): void {
      if (paragraph.length > 0) {
        out.push(`<p>${renderInline(paragraph.join(' '))}</p>`)
        paragraph = []
      }
    }
    function flushList(): void {
      if (list.length > 0) {
        out.push(`<ul>${list.map((item) => `<li>${renderInline(item)}</li>`).join('')}</ul>`)
        list = []
      }
    }

    let i = 0
    while (i < lines.length) {
      const line = lines[i]
      if (line.startsWith('```')) {
        flushParagraph()
        flushList()
        const codeLines: string[] = []
        i++
        while (i < lines.length && !lines[i].startsWith('```')) {
          codeLines.push(lines[i])
          i++
        }
        out.push(`<pre><code>${codeLines.join('\n')}</code></pre>`)
        i++ // skip closing fence
        continue
      }
      if (line.startsWith('- ')) {
        flushParagraph()
        list.push(line.slice(2))
        i++
        continue
      }
      if (line.trim() === '') {
        flushParagraph()
        flushList()
        i++
        continue
      }
      flushList()
      paragraph.push(line.trim())
      i++
    }
    flushParagraph()
    flushList()
    return out.join('')
  }

  const submitTitle = $derived(
    rs.allReviewed
      ? 'Submit review'
      : `Every concern needs a verdict (${(rs.session?.concerns.length ?? 0) - rs.reviewedCount} remaining)`,
  )
</script>

<svelte:window onkeydown={handleKeydown} />

{#if rs.phase === 'loading'}
  <div class="center-message">Loading…</div>
{:else if rs.phase === 'error'}
  <div class="center-message">Session not found or already finished.</div>
{:else if rs.phase === 'submitted'}
  <div class="center-message">Review submitted. You can close this tab.</div>
{:else if rs.phase === 'aborted'}
  <div class="center-message">Review aborted. You can close this tab.</div>
{:else if rs.session}
  <div class="app">
    <header class="topbar">
      <div class="topbar-title">
        <h1>{rs.session.title}</h1>
        {#if rs.session.summary}
          <p class="summary">{rs.session.summary}</p>
        {/if}
      </div>
      <div class="topbar-actions">
        <span class="reviewed-counter"
          >{rs.reviewedCount}/{rs.session.concerns.length} reviewed</span
        >
        <button
          type="button"
          disabled={!rs.allReviewed}
          title={submitTitle}
          onclick={openSubmitConfirm}>Submit review</button
        >
        <button type="button" onclick={openAbortConfirm}>Abort review</button>
      </div>
    </header>
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
            <li>{warning}</li>
          {/each}
        </ul>
      </div>
    {/if}
    <div class="body">
      <aside class="left-pane">
        <ConcernList />
      </aside>
      <main class="main-pane">
        {#if rs.selected}
          {@const selected = rs.selected}
          {@const selectedComments = rs.draft.concerns[selected.id]?.comments ?? []}
          <div class="concern-header">
            <h2>{selected.title}</h2>
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
                <li><span class="comment-loc">{comment.path}:{comment.line}</span> {comment.body}</li>
              {/each}
            </ul>
          {/if}
          {#if selected.description}
            <div class="concern-description">{@html renderMarkdown(selected.description)}</div>
          {/if}
          <DiffView />
        {/if}
        <section class="general-comments">
          <h3>General comments</h3>
          <textarea
            id="general-comment-textarea"
            bind:value={generalCommentText}
            placeholder="Add a general comment…"
            rows="3"
          ></textarea>
          <button
            type="button"
            onclick={addGeneralComment}
            disabled={!generalCommentText.trim()}>Add</button
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
  </div>

  {#if showSubmitConfirm}
    <div class="modal-overlay">
      <div class="modal-panel">
        <h2>Submit review</h2>
        <ul class="verdict-summary">
          {#each rs.session.concerns as c (c.id)}
            <li>
              <span class="vs-title">{c.title}</span>
              <span class="vs-verdict">{verdictLabel(rs.draft.concerns[c.id]?.verdict)}</span>
            </li>
          {/each}
        </ul>
        {#if rs.submitError}
          <p class="modal-error">{rs.submitError}</p>
        {/if}
        <div class="modal-actions">
          <button type="button" onclick={confirmSubmit} disabled={rs.submitting}
            >{rs.submitting ? 'Submitting…' : 'Confirm submit'}</button
          >
          <button
            type="button"
            onclick={() => (showSubmitConfirm = false)}
            disabled={rs.submitting}>Cancel</button
          >
        </div>
      </div>
    </div>
  {/if}

  {#if showAbortConfirm}
    <div class="modal-overlay">
      <div class="modal-panel">
        <p>Abort review? The agent will receive exit code 2 and no result.</p>
        {#if rs.submitError}
          <p class="modal-error">{rs.submitError}</p>
        {/if}
        <div class="modal-actions">
          <button type="button" onclick={confirmAbort} disabled={rs.submitting}
            >{rs.submitting ? 'Aborting…' : 'Confirm abort'}</button
          >
          <button type="button" onclick={() => (showAbortConfirm = false)} disabled={rs.submitting}
            >Cancel</button
          >
        </div>
      </div>
    </div>
  {/if}

  <footer class="shortcut-hint">
    j/k select · a approve · x request changes · c comment · Enter submit · i comment box · Esc close
  </footer>
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
    border-bottom: 1px solid #e2e2e2;
    background: #fafafa;
    flex-wrap: wrap;
  }

  .topbar-title h1 {
    margin: 0;
    font-size: 16px;
  }

  .topbar-title .summary {
    margin: 2px 0 0;
    font-size: 12px;
    color: #666;
    max-width: 60ch;
  }

  .topbar-actions {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .reviewed-counter {
    font-size: 13px;
    color: #444;
    font-variant-numeric: tabular-nums;
  }

  .topbar-actions button {
    padding: 6px 12px;
    border: 1px solid #ccc;
    border-radius: 4px;
    background: #fff;
    font-size: 13px;
    color: #333;
    cursor: pointer;
  }

  .topbar-actions button:hover:not(:disabled) {
    background: #f0f1f3;
  }

  .topbar-actions button:disabled {
    color: #999;
    cursor: not-allowed;
  }

  .warnings-banner {
    padding: 8px 16px;
    background: #fff4e5;
    border-bottom: 1px solid #f0dca6;
    color: #9a6700;
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
    letter-spacing: 0.02em;
  }

  .warnings-banner-dismiss {
    flex-shrink: 0;
    border: none;
    background: none;
    color: #9a6700;
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
  }

  .body {
    flex: 1;
    display: flex;
    min-height: 0;
  }

  .left-pane {
    width: 280px;
    flex: 0 0 280px;
    border-right: 1px solid #e2e2e2;
    overflow-y: auto;
    background: #fafafa;
  }

  .main-pane {
    flex: 1;
    overflow-y: auto;
    padding: 16px 20px;
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
    font-size: 18px;
  }

  .concern-description {
    font-size: 14px;
    line-height: 1.5;
    color: #333;
    margin-bottom: 16px;
    max-width: 80ch;
  }

  .concern-description :global(p) {
    margin: 0 0 10px;
  }

  .concern-description :global(ul) {
    margin: 0 0 10px 20px;
    padding: 0;
  }

  .concern-description :global(code) {
    background: #f0f0f0;
    padding: 1px 4px;
    border-radius: 3px;
    font-family: ui-monospace, 'SF Mono', Menlo, Consolas, monospace;
    font-size: 13px;
  }

  .concern-description :global(pre) {
    background: #f6f8fa;
    padding: 10px;
    border-radius: 4px;
    overflow-x: auto;
  }

  .concern-description :global(pre code) {
    background: none;
    padding: 0;
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
    color: #333;
    background: #fff8c5;
    border: 1px solid #d4c76a;
    border-radius: 4px;
    padding: 6px 10px;
  }

  .comment-loc {
    font-family: ui-monospace, 'SF Mono', Menlo, Consolas, monospace;
    font-size: 12px;
    color: #666;
    margin-right: 6px;
  }

  .general-comments {
    margin-top: 32px;
    padding-top: 16px;
    border-top: 1px solid #e2e2e2;
    max-width: 80ch;
  }

  .general-comments h3 {
    margin: 0 0 10px;
    font-size: 15px;
  }

  .general-comments textarea {
    width: 100%;
    box-sizing: border-box;
    font-family: inherit;
    font-size: 13px;
    padding: 8px;
    border: 1px solid #ccc;
    border-radius: 4px;
    resize: vertical;
  }

  .general-comments > button {
    margin-top: 8px;
    padding: 6px 14px;
    border: 1px solid #0969da;
    border-radius: 4px;
    background: #0969da;
    color: #fff;
    font-size: 13px;
    cursor: pointer;
  }

  .general-comments > button:disabled {
    background: #a8c8e8;
    border-color: #a8c8e8;
    cursor: not-allowed;
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
    color: #333;
    background: #f6f8fa;
    border: 1px solid #d0d7de;
    border-radius: 4px;
    padding: 8px 12px;
    white-space: pre-wrap;
  }

  .comment-delete {
    flex-shrink: 0;
    border: none;
    background: none;
    color: #666;
    cursor: pointer;
    font-size: 14px;
    line-height: 1;
    padding: 0 2px;
  }

  .comment-delete:hover {
    color: #cf222e;
  }

  .center-message {
    display: flex;
    align-items: center;
    justify-content: center;
    height: 100vh;
    font-size: 15px;
    color: #444;
  }

  .modal-overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.4);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 10;
  }

  .modal-panel {
    background: #fff;
    border-radius: 8px;
    padding: 20px 24px;
    max-width: 440px;
    width: 90%;
    max-height: 80vh;
    overflow-y: auto;
    box-shadow: 0 8px 30px rgba(0, 0, 0, 0.2);
  }

  .modal-panel h2 {
    margin: 0 0 12px;
    font-size: 16px;
  }

  .modal-panel p {
    margin: 0 0 12px;
    font-size: 14px;
    color: #333;
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
    border-bottom: 1px solid #eee;
  }

  .vs-title {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .vs-verdict {
    flex-shrink: 0;
    color: #666;
  }

  .modal-error {
    font-size: 13px;
    color: #cf222e;
    background: #ffebe9;
    border: 1px solid #ffc1bc;
    border-radius: 4px;
    padding: 8px 10px;
    margin: 0 0 12px;
  }

  .modal-actions {
    display: flex;
    gap: 10px;
    justify-content: flex-end;
  }

  .modal-actions button {
    padding: 6px 14px;
    border: 1px solid #ccc;
    border-radius: 4px;
    background: #fff;
    font-size: 13px;
    cursor: pointer;
  }

  .modal-actions button:first-child {
    background: #0969da;
    border-color: #0969da;
    color: #fff;
  }

  .modal-actions button:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .shortcut-hint {
    position: fixed;
    left: 0;
    right: 0;
    bottom: 0;
    padding: 3px 16px;
    font-size: 11px;
    color: #888;
    background: rgba(250, 250, 250, 0.9);
    border-top: 1px solid #e2e2e2;
    text-align: center;
    pointer-events: none;
    z-index: 5;
  }
</style>
