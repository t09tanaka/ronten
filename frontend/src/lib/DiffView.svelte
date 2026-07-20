<script lang="ts">
  import { rs } from './state.svelte'
  import HunkView from './HunkView.svelte'
  import type { CommentLineInfo } from './anchors'
  import type { FileStatus, HunkRef } from './types'

  interface FileGroup {
    fileIndex: number
    refs: HunkRef[]
  }

  // Concern.hunks is already sorted/deduped by (file, hunk) on the server
  // (see mapping.rs), so a single left-to-right pass groups consecutive
  // refs sharing a file without needing to re-sort.
  const groups = $derived.by((): FileGroup[] => {
    const concern = rs.selected
    if (!concern) return []
    const out: FileGroup[] = []
    let current: FileGroup | null = null
    for (const ref of concern.hunks) {
      if (!current || current.fileIndex !== ref.file) {
        current = { fileIndex: ref.file, refs: [] }
        out.push(current)
      }
      current.refs.push(ref)
    }
    return out
  })

  function statusCardText(status: FileStatus): string {
    switch (status) {
      case 'binary':
        return 'Binary file changed'
      case 'non-utf8':
        return 'Non-UTF-8 file changed (content not displayed)'
      case 'too-large':
        return 'File too large to display'
      case 'renamed':
        return 'File renamed (no content changes)'
      case 'added':
        return 'Empty file added'
      case 'deleted':
        return 'Empty file deleted'
      default:
        return 'File changed (no content changes)'
    }
  }

  function handleCommentLine(info: CommentLineInfo): void {
    // Clicking the same line again re-opens the same target — toggle it
    // closed instead of leaving a no-op editor mount in place.
    const p = rs.pendingCommentTarget
    if (p != null && p.path === info.path && p.side === info.side && p.line === info.line) {
      rs.pendingCommentTarget = null
    } else {
      rs.pendingCommentTarget = info
    }
  }
</script>

{#if !rs.selected}
  <p class="empty-diff">No concern selected.</p>
{:else if groups.length === 0}
  <p class="empty-diff">No changes mapped to this concern.</p>
{:else if rs.session}
  {#each groups as group (group.fileIndex)}
    {@const file = rs.session.files[group.fileIndex]}
    <section class="file-group">
      <header class="file-header">
        {#if file.old_path && file.new_path && file.old_path !== file.new_path}
          <span class="file-path">{file.old_path} → {file.new_path}</span>
        {:else}
          <span class="file-path">{file.new_path ?? file.old_path}</span>
        {/if}
        <span class="file-status status-{file.status}">{file.status}</span>
      </header>
      {#each group.refs as hunkRef (hunkRef.hunk ?? -1)}
        {#if hunkRef.hunk === null}
          <div class="status-card">{statusCardText(file.status)}</div>
        {:else}
          <HunkView
            {file}
            hunk={file.hunks[hunkRef.hunk]}
            {hunkRef}
            concernId={rs.selected.id}
            oncommentline={handleCommentLine}
          />
        {/if}
      {/each}
    </section>
  {/each}
{/if}

<style>
  .file-group {
    margin-bottom: 24px;
  }

  .file-header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 10px;
    background: var(--c-panel);
    border: 1px solid var(--c-rule);
    border-bottom: none;
    border-radius: 4px 4px 0 0;
    font-family: var(--font-mono);
    font-size: 13px;
  }

  .file-path {
    font-weight: 600;
  }

  .file-status {
    font-size: 11px;
    text-transform: uppercase;
    color: var(--c-ink-2);
    background: var(--c-neutral-tint);
    padding: 1px 6px;
    border-radius: 3px;
  }

  .status-card {
    padding: 10px;
    border: 1px solid var(--c-rule);
    border-top: none;
    font-size: 13px;
    color: var(--c-ink-2);
    font-style: italic;
  }

  .empty-diff {
    color: var(--c-ink-2);
    font-size: 14px;
  }
</style>
