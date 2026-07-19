<script lang="ts">
  import { rs } from './state.svelte'
  import HunkView from './HunkView.svelte'
  import type { HunkRef, Side } from './types'

  interface FileGroup {
    fileIndex: number
    refs: HunkRef[]
  }

  interface CommentLineInfo {
    path: string
    side: Side
    line: number
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

  function statusCardText(status: string): string {
    return status === 'binary' ? 'Binary file changed' : 'File renamed (no content changes)'
  }

  function handleCommentLine(info: CommentLineInfo): void {
    // Wired up fully in Task 12; for now just log so the callback path is
    // exercised end-to-end.
    console.log('comment line', info)
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
        <span class="file-path">{file.new_path ?? file.old_path}</span>
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
    background: #f6f8fa;
    border: 1px solid #e2e2e2;
    border-bottom: none;
    border-radius: 6px 6px 0 0;
    font-family: ui-monospace, 'SF Mono', Menlo, Consolas, monospace;
    font-size: 13px;
  }

  .file-path {
    font-weight: 600;
  }

  .file-status {
    font-size: 11px;
    text-transform: uppercase;
    color: #57606a;
    background: #eef0f2;
    padding: 1px 6px;
    border-radius: 3px;
  }

  .status-card {
    padding: 10px;
    border: 1px solid #e2e2e2;
    border-top: none;
    font-size: 13px;
    color: #666;
    font-style: italic;
  }

  .empty-diff {
    color: #666;
    font-size: 14px;
  }
</style>
