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
    <button type="button" onclick={add} disabled={!body.trim()}>Add comment</button>
    <button type="button" onclick={cancel}>Cancel</button>
  </div>
</div>

<style>
  .comment-editor {
    padding: 10px;
    background: #f6f8fa;
    border: 1px solid #d0d7de;
    border-radius: 4px;
    margin: 4px 0;
  }

  .comment-editor textarea {
    width: 100%;
    box-sizing: border-box;
    font-family: inherit;
    font-size: 13px;
    padding: 8px;
    border: 1px solid #ccc;
    border-radius: 4px;
    resize: vertical;
  }

  .comment-editor-actions {
    display: flex;
    gap: 8px;
    margin-top: 8px;
  }

  .comment-editor-actions button {
    padding: 5px 12px;
    border: 1px solid #ccc;
    border-radius: 4px;
    background: #fff;
    font-size: 13px;
    cursor: pointer;
  }

  .comment-editor-actions button:first-child {
    background: #0969da;
    border-color: #0969da;
    color: #fff;
  }

  .comment-editor-actions button:first-child:disabled {
    background: #a8c8e8;
    border-color: #a8c8e8;
    cursor: not-allowed;
  }
</style>
