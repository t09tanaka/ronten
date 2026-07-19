<script lang="ts">
  import { rs } from './state.svelte'
  import type { Side } from './types'

  interface Props {
    concernId: string
    path: string
    side: Side
    line: number
  }

  let { concernId, path, side, line }: Props = $props()

  let body = $state('')

  function add(): void {
    const trimmed = body.trim()
    if (!trimmed) return
    rs.addComment(concernId, { path, side, line, body: trimmed })
    rs.pendingCommentTarget = null
  }

  function cancel(): void {
    rs.pendingCommentTarget = null
  }
</script>

<div class="comment-editor">
  <!-- svelte-ignore a11y_autofocus -->
  <textarea
    bind:value={body}
    placeholder="Add a comment…"
    rows="3"
    autofocus
  ></textarea>
  <div class="comment-editor-actions">
    <button type="button" class="editor-add" onclick={add} disabled={!body.trim()}>Add comment</button>
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
    border: 1px solid var(--c-rule);
    border-radius: 4px;
    background: var(--c-paper);
    color: var(--c-ink);
    resize: vertical;
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
    background: #efece3;
    color: var(--c-ink);
  }
</style>
