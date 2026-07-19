<script lang="ts">
  import { rs } from './state.svelte'
  import type { ConcernView, DiffLine, FileDiff, Hunk, HunkRef, Side } from './types'

  const COLLAPSE_THRESHOLD = 200

  interface CommentLineInfo {
    path: string
    side: Side
    line: number
  }

  interface Props {
    file: FileDiff
    hunk: Hunk
    hunkRef: HunkRef
    concernId: string
    oncommentline: (info: CommentLineInfo) => void
  }

  let { file, hunk, hunkRef, concernId, oncommentline }: Props = $props()

  // Deliberately captures only the initial value: each hunk gets its own
  // component instance (keyed by hunkRef.hunk in DiffView), so this seeds
  // the starting collapsed state once per hunk; `collapsed` itself is a
  // separate piece of state the user toggles afterward.
  // svelte-ignore state_referenced_locally
  let collapsed = $state(hunk.lines.length > COLLAPSE_THRESHOLD)

  const otherOwners = $derived(
    rs
      .hunkOwners(hunkRef)
      .filter((id) => id !== concernId)
      .map((id) => rs.session?.concerns.find((c) => c.id === id))
      .filter((c): c is ConcernView => c != null),
  )

  function lineClick(line: DiffLine): void {
    const path = file.new_path ?? file.old_path
    if (!path) return
    if (line.kind === 'remove') {
      if (line.old_no == null) return
      oncommentline({ path, side: 'old', line: line.old_no })
    } else {
      if (line.new_no == null) return
      oncommentline({ path, side: 'new', line: line.new_no })
    }
  }

  function jumpTo(id: string): void {
    const idx = rs.session?.concerns.findIndex((c) => c.id === id) ?? -1
    if (idx >= 0) rs.select(idx)
  }
</script>

<div class="hunk">
  <div class="hunk-header">
    <span class="hunk-range"
      >@@ -{hunk.old_start},{hunk.old_count} +{hunk.new_start},{hunk.new_count} @@</span
    >
    {#if hunk.section}
      <span class="hunk-section">{hunk.section}</span>
    {/if}
    {#if otherOwners.length > 0}
      <span class="shared-badge">
        shared with
        {#each otherOwners as owner, i (owner.id)}
          <button type="button" class="owner-link" onclick={() => jumpTo(owner.id)}
            >{owner.title}</button
          >{i < otherOwners.length - 1 ? ',' : ''}
        {/each}
      </span>
    {/if}
  </div>

  {#if collapsed}
    <button
      type="button"
      class="collapse-toggle"
      title="Expand"
      aria-label="Expand"
      onclick={() => (collapsed = false)}
    >
      &#9656; {hunk.lines.length} lines &mdash; click to expand
    </button>
  {:else}
    <div class="hunk-body">
      <table class="hunk-table">
        <tbody>
          {#each hunk.lines as line, i (i)}
            <tr class="line line-{line.kind}">
              <td class="gutter old-gutter" onclick={() => lineClick(line)}
                >{line.old_no ?? ''}</td
              >
              <td class="gutter new-gutter" onclick={() => lineClick(line)}
                >{line.new_no ?? ''}</td
              >
              <td class="content">{line.content}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
    {#if hunk.lines.length > COLLAPSE_THRESHOLD}
      <button type="button" class="collapse-toggle" onclick={() => (collapsed = true)}
        >Collapse</button
      >
    {/if}
  {/if}
</div>

<style>
  .hunk {
    border: 1px solid #e2e2e2;
    border-top: none;
  }

  .hunk-header {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 4px 10px;
    background: #fafafa;
    color: #8b949e;
    font-family: ui-monospace, 'SF Mono', Menlo, Consolas, monospace;
    font-size: 12px;
    border-bottom: 1px solid #eee;
    flex-wrap: wrap;
  }

  .hunk-range {
    color: #6e7781;
  }

  .hunk-section {
    color: #999;
  }

  .shared-badge {
    margin-left: auto;
    font-size: 11px;
    color: #9a6700;
    display: flex;
    align-items: center;
    gap: 3px;
  }

  .owner-link {
    background: none;
    border: none;
    color: #0969da;
    cursor: pointer;
    padding: 0;
    font-size: 11px;
    text-decoration: underline;
    font-family: inherit;
  }

  .collapse-toggle {
    display: block;
    width: 100%;
    text-align: left;
    padding: 8px 10px;
    background: #f6f8fa;
    border: none;
    border-top: 1px solid #eee;
    color: #57606a;
    cursor: pointer;
    font-family: ui-monospace, 'SF Mono', Menlo, Consolas, monospace;
    font-size: 12px;
  }

  .hunk-body {
    overflow-x: auto;
  }

  .hunk-table {
    width: 100%;
    border-collapse: collapse;
    font-family: ui-monospace, 'SF Mono', Menlo, Consolas, monospace;
    font-size: 12.5px;
  }

  .gutter {
    width: 1%;
    white-space: nowrap;
    padding: 0 8px;
    text-align: right;
    color: #8b949e;
    user-select: none;
    cursor: pointer;
    background: #fafbfc;
  }

  .gutter:hover {
    background: #eaeef2;
  }

  .content {
    padding: 0 10px;
    white-space: pre;
  }

  .line-add {
    background: #e6ffec;
  }

  .line-remove {
    background: #ffebe9;
  }
</style>
