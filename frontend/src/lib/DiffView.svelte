<script lang="ts">
  import { rs } from './state.svelte'
  import { shouldCollapseAllHunks } from './collapse'
  import HunkView from './HunkView.svelte'
  import type { CommentLineInfo } from './anchors'
  import { hasControlChars, hasInvisibles, reveal } from './invisibles'
  import { contentNote, fileNotices, modeChangeBadge, opaqueDetails, requiresAck, typeChangeBadge } from './opaque'
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

  // Sum of rendered lines across every hunk the SELECTED concern owns
  // (not just the ones in a single file group) — the input to the
  // many-small-hunks collapse rule. A concern with hundreds of small hunks
  // can build unbounded DOM even though no individual hunk crosses
  // HunkView's own per-hunk threshold; see collapse.ts.
  const selectedTotalLines = $derived.by((): number => {
    const concern = rs.selected
    const session = rs.session
    if (!concern || !session) return 0
    let total = 0
    for (const ref of concern.hunks) {
      if (ref.hunk == null) continue
      total += session.files[ref.file].hunks[ref.hunk].lines.length
    }
    return total
  })

  const forceCollapseAll = $derived(shouldCollapseAllHunks(selectedTotalLines))

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
    // Paths use the TAB-including check (matches revealControlChars, which
    // is what actually renders them); line content uses the TAB-excluding
    // check (matches revealInvisibles) so an ordinary tab-indented line
    // doesn't trip the badge when no token would actually be revealed.
    if (file.old_path != null && hasControlChars(file.old_path)) return true
    if (file.new_path != null && hasControlChars(file.new_path)) return true
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
  {#if rs.selected.unmapped && rs.session.unmapped_lines.length > 0}
    <p class="unmapped-legend">Highlighted lines were not assigned to any concern.</p>
  {/if}
  <!-- Keyed on the selected concern id so switching concerns forces a full
       remount of every HunkView below, not just a props update. Without
       this, `{#each group.refs as hunkRef (hunkRef.hunk ?? -1)}` keys purely
       on (file, hunk) — a hunk shared by two concerns keeps the SAME
       HunkView instance across a concern switch, so its `collapsed` $state
       (seeded once from `forceCollapsed`/the per-hunk threshold, see
       HunkView.svelte) never re-evaluates: a shared hunk could stay
       expanded under a concern whose total exceeds the force-collapse
       threshold, silently escaping the DOM bound this feature exists to
       enforce. Remounting on every concern switch does mean a manually
       expanded/collapsed hunk resets when you switch away and back — that's
       fine, all persisted state (comments, verdicts, buffers) lives in the
       store, not in HunkView. -->
  {#key rs.selected.id}
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
          {#if modeChangeBadge(file)}
            <span class="file-status kind-meta">{modeChangeBadge(file)}</span>
          {/if}
          {#if typeChangeBadge(file)}
            <span class="file-status kind-meta">{typeChangeBadge(file)}</span>
          {/if}
          {#if fileHasInvisibles(file, group.refs)}
            <span
              class="file-status kind-invisible"
              title="Contains invisible or bidirectional Unicode characters (revealed inline as ⟨U+XXXX⟩)"
              >hidden unicode</span
            >
          {/if}
        </header>
        {#each fileNotices(file) as notice (notice)}
          <p class="file-notice">{notice}</p>
        {/each}
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
              forceCollapsed={forceCollapseAll}
              oncommentline={handleCommentLine}
            />
          {/if}
        {/each}
        <!-- Text files can still require an ack (gitlink, mode change — see
             requiresAck): the header badges above explain why, and the same
             ack checkbox as the opaque card gates submission. Opaque files
             already carry theirs inside the opaque card. -->
        {#if file.content_kind === 'text' && requiresAck(file)}
          <div class="opaque-card">
            <label class="opaque-ack">
              <input
                type="checkbox"
                checked={rs.isAcked(group.fileIndex)}
                disabled={rs.phase !== 'review'}
                onchange={() => rs.toggleAck(group.fileIndex)}
              />
              I acknowledge this change
            </label>
          </div>
        {/if}
      </section>
    {/each}
  {/key}
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
       codepoints are also revealed as ⟨U+XXXX⟩ tokens by revealControlChars). */
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

  /* mode/type transition badges carry literal values (git modes, type
     names) — keep their original casing. */
  .kind-meta {
    text-transform: none;
  }

  .file-notice {
    margin: 0;
    padding: 6px 10px;
    border: 1px solid var(--c-rule);
    border-top: none;
    border-bottom: none;
    background: var(--c-odo-tint);
    color: var(--c-odo);
    font-size: 12px;
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
