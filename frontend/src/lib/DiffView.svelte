<script lang="ts">
  import { rs } from './state.svelte'
  import HunkView from './HunkView.svelte'
  import type { CommentLineInfo } from './anchors'
  import { hasInvisibles, reveal } from './invisibles'
  import { contentNote, opaqueDetails } from './opaque'
  import type { ChangeKind, FileDiff, HunkRef } from './types'

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

  // Only called for content_kind === 'text' files with no hunks — opaque
  // files get the detail card + ack checkbox below instead.
  function statusCardText(file: FileDiff): string {
    const changeKind: ChangeKind = file.change_kind
    if (changeKind === 'added') return 'Empty file added'
    if (changeKind === 'deleted') return 'Empty file deleted'
    if (changeKind === 'renamed') return 'File renamed (no content changes)'
    if (file.old_mode != null && file.new_mode != null && file.old_mode !== file.new_mode) {
      return 'File mode changed'
    }
    return 'File changed (no content changes)'
  }

  // Trojan Source defense (see invisibles.ts): a badge on the file header so
  // a reviewer knows to look for the revealed ⟨U+XXXX⟩ tokens, driven off
  // either path or any displayed hunk line for this concern.
  function fileHasInvisibles(file: FileDiff, refs: HunkRef[]): boolean {
    if (file.old_path != null && hasInvisibles(file.old_path)) return true
    if (file.new_path != null && hasInvisibles(file.new_path)) return true
    for (const ref of refs) {
      if (ref.hunk == null) continue
      if (file.hunks[ref.hunk].lines.some((line) => hasInvisibles(line.content))) return true
    }
    return false
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
  {#if rs.selected.unmapped}
    <p class="unmapped-legend">Highlighted lines are changes no concern claimed.</p>
  {/if}
  {#each groups as group (group.fileIndex)}
    {@const file = rs.session.files[group.fileIndex]}
    <section class="file-group">
      <header class="file-header">
        {#if file.old_path && file.new_path && file.old_path !== file.new_path}
          <span class="file-path">{reveal(file.old_path)} → {reveal(file.new_path)}</span>
        {:else}
          <span class="file-path">{reveal(file.new_path ?? file.old_path)}</span>
        {/if}
        <span class="file-status kind-{file.change_kind}">{file.change_kind}</span>
        {#if file.content_kind !== 'text'}
          <span class="file-status kind-{file.content_kind}">{file.content_kind}</span>
        {/if}
        {#if fileHasInvisibles(file, group.refs)}
          <span
            class="file-status kind-invisible"
            title="Contains invisible or bidirectional Unicode characters (revealed inline as ⟨U+XXXX⟩)"
            >hidden unicode</span
          >
        {/if}
      </header>
      {#each group.refs as hunkRef (hunkRef.hunk ?? -1)}
        {#if hunkRef.hunk === null}
          {#if file.content_kind === 'text'}
            <div class="status-card">{statusCardText(file)}</div>
          {:else}
            <div class="opaque-card">
              <p class="opaque-note">{contentNote(file.content_kind)}</p>
              {#if opaqueDetails(file).length > 0}
                <dl class="opaque-details">
                  {#each opaqueDetails(file) as row (row.label)}
                    <dt>{row.label}</dt>
                    <dd>{row.value}</dd>
                  {/each}
                </dl>
              {/if}
              <label class="opaque-ack">
                <input
                  type="checkbox"
                  checked={rs.isAcked(group.fileIndex)}
                  disabled={rs.phase !== 'review'}
                  onchange={() => rs.toggleAck(group.fileIndex)}
                />
                I acknowledge this change without reviewing its contents
              </label>
            </div>
          {/if}
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
    /* Trojan Source defense: pin display order to logical order so bidi
       control characters in the path can't reorder how it renders (their
       codepoints are also revealed as ⟨U+XXXX⟩ tokens by revealInvisibles). */
    unicode-bidi: isolate;
    direction: ltr;
  }

  .file-status {
    font-size: 11px;
    text-transform: uppercase;
    color: var(--c-ink-2);
    background: var(--c-neutral-tint);
    padding: 1px 6px;
    border-radius: 3px;
  }

  .kind-invisible {
    color: var(--c-odo);
    background: var(--c-odo-tint);
  }

  .status-card {
    padding: 10px;
    border: 1px solid var(--c-rule);
    border-top: none;
    font-size: 13px;
    color: var(--c-ink-2);
    font-style: italic;
  }

  .opaque-card {
    padding: 10px;
    border: 1px solid var(--c-rule);
    border-top: none;
    font-size: 13px;
    background: var(--c-odo-tint);
  }

  .opaque-note {
    margin: 0 0 8px;
    color: var(--c-odo);
    font-weight: 600;
  }

  .opaque-details {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 2px 10px;
    margin: 0 0 10px;
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--c-ink-2);
  }

  .opaque-details dt {
    font-weight: 600;
  }

  .opaque-details dd {
    margin: 0;
  }

  .opaque-ack {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 13px;
    color: var(--c-ink);
    cursor: pointer;
  }

  .empty-diff {
    color: var(--c-ink-2);
    font-size: 14px;
  }

  .unmapped-legend {
    margin: 0 0 10px;
    padding: 6px 10px;
    border: 1px solid var(--c-odo);
    border-radius: 4px;
    background: var(--c-odo-tint);
    color: var(--c-odo);
    font-size: 12.5px;
  }
</style>
