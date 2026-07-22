<script lang="ts">
  import { commentTargetKey, rs } from './state.svelte'
  import { scalarLength, truncateToScalars } from './textLimits'
  import type { Side } from './types'

  interface Props {
    concernId: string
    path: string
    side: Side
    line: number
  }

  let { concernId, path, side, line }: Props = $props()

  // Backed by the central store (not local $state) so the in-progress text
  // survives a concern switch or the editor being closed and reopened —
  // see Task 2.5 / P1-5.
  const key = $derived(commentTargetKey(concernId, { path, side, line }))
  const body = $derived(rs.editorBuffer(key))

  const maxChars = $derived(rs.limits?.max_comment_chars)

  // Truncates by Unicode scalar count, matching the server's
  // `chars().count()` — NOT the native `maxlength` attribute, which counts
  // UTF-16 code units and could split an astral character's surrogate pair
  // (P1-6).
  function setBody(v: string): void {
    rs.setEditorBuffer(key, maxChars != null ? truncateToScalars(v, maxChars) : v)
  }

  function add(): void {
    const trimmed = body.trim()
    if (!trimmed) return
    rs.addComment(concernId, { path, side, line, body: trimmed })
    rs.clearEditorBuffer(key)
    rs.pendingCommentTarget = null
  }

  function cancel(): void {
    rs.pendingCommentTarget = null
  }
</script>

<div class="comment-editor">
  <!-- svelte-ignore a11y_autofocus -->
  <textarea
    bind:value={() => body, setBody}
    placeholder="Add a comment…"
    rows="3"
    autofocus
  ></textarea>
  {#if maxChars != null && scalarLength(body) > maxChars * 0.9}
    <span class="char-count">{scalarLength(body)}/{maxChars}</span>
  {/if}
  <div class="comment-editor-actions">
    <button
      type="button"
      class="editor-add"
      onclick={add}
      disabled={!body.trim() || rs.draftByteBlocked}>Add comment</button
    >
    <button type="button" class="editor-cancel" onclick={cancel}>Cancel</button>
  </div>
</div>

<style>
  .comment-editor {
    padding: 10px;
    background: var(--c-panel);
    border: 1px solid var(--c-rule);
    border-radius: 4px;
    margin: 4px 0;
  }

  .comment-editor textarea {
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

  .char-count {
    display: block;
    margin-top: 2px;
    font-size: 11px;
    color: var(--c-ink-3);
    text-align: right;
    font-variant-numeric: tabular-nums;
  }

  .comment-editor-actions {
    display: flex;
    gap: 8px;
    margin-top: 8px;
  }

  .editor-add {
    padding: 5px 12px;
    border: 1px solid var(--c-ai);
    border-radius: 3px;
    background: var(--c-paper);
    color: var(--c-ai);
    font-size: 13px;
    font-family: inherit;
    cursor: pointer;
  }

  .editor-add:hover:not(:disabled) {
    background: var(--c-ai-tint);
  }

  .editor-add:disabled {
    border-color: var(--c-rule);
    color: var(--c-ink-3);
    cursor: not-allowed;
  }

  .editor-cancel {
    padding: 5px 12px;
    border: 1px solid var(--c-rule);
    border-radius: 3px;
    background: transparent;
    color: var(--c-ink-2);
    font-size: 13px;
    font-family: inherit;
    cursor: pointer;
  }

  .editor-cancel:hover {
    background: var(--c-hover-wash);
    color: var(--c-ink);
  }
</style>
