<script lang="ts">
  import { rs } from './state.svelte'
  import CommentEditor from './CommentEditor.svelte'
  import type { Comment, ConcernView, DiffLine, FileDiff, Hunk, HunkRef, Side } from './types'

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

  const path = $derived(file.new_path ?? file.old_path)
  const comments = $derived(rs.draft.concerns[concernId]?.comments ?? [])

  // Mirrors lineClick's side/line selection without firing the click
  // callback, so saved comments and the pending editor can be located
  // under the right row.
  function lineTarget(line: DiffLine): CommentLineInfo | null {
    if (!path) return null
    if (line.kind === 'remove') {
      return line.old_no != null ? { path, side: 'old', line: line.old_no } : null
    }
    return line.new_no != null ? { path, side: 'new', line: line.new_no } : null
  }

  function commentsFor(target: CommentLineInfo): Comment[] {
    return comments.filter(
      (c) => c.path === target.path && c.side === target.side && c.line === target.line,
    )
  }

  function isPending(target: CommentLineInfo): boolean {
    const p = rs.pendingCommentTarget
    return p != null && p.path === target.path && p.side === target.side && p.line === target.line
  }

  function deleteComment(comment: Comment): void {
    const all = rs.draft.concerns[concernId]?.comments
    if (!all) return
    const idx = all.indexOf(comment)
    if (idx >= 0) rs.removeComment(concernId, idx)
  }

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
      ▸ {hunk.lines.length} lines — click to expand
    </button>
  {:else}
    <div class="hunk-body">
      <table class="hunk-table">
        <tbody>
          {#each hunk.lines as line, i (i)}
            {@const target = lineTarget(line)}
            <tr class="line line-{line.kind}">
              <td class="gutter old-gutter" onclick={() => lineClick(line)}
                >{line.old_no ?? ''}</td
              >
              <td class="gutter new-gutter" onclick={() => lineClick(line)}
                >{line.new_no ?? ''}</td
              >
              <td class="content">{line.content}</td>
            </tr>
            {#if target}
              {#each commentsFor(target) as comment (comment)}
                <tr class="comment-row">
                  <td colspan="3">
                    <div class="inline-anchor">
                      <div class="comment-block">
                        <span class="comment-body">{comment.body}</span>
                        <button
                          type="button"
                          class="comment-delete"
                          aria-label="Delete comment"
                          onclick={() => deleteComment(comment)}>×</button
                        >
                      </div>
                    </div>
                  </td>
                </tr>
              {/each}
              {#if isPending(target)}
                <tr class="comment-editor-row">
                  <td colspan="3">
                    <div class="inline-anchor">
                      <CommentEditor
                        {concernId}
                        path={target.path}
                        side={target.side}
                        line={target.line}
                      />
                    </div>
                  </td>
                </tr>
              {/if}
            {/if}
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
    /* Inline-size container so .inline-anchor can size itself against the
       visible scrollport width (100cqw) instead of the full table width. */
    container-type: inline-size;
  }

  .hunk-table {
    /* Shared fixed gutter width: the second sticky gutter's `left` offset
       must equal the first gutter's width, so both come from one value. */
    --gutter-w: 3.5em;
    width: 100%;
    border-collapse: collapse;
    font-family: ui-monospace, 'SF Mono', Menlo, Consolas, monospace;
    font-size: 12.5px;
  }

  /* Gutters stay pinned while the code content scrolls horizontally, so the
     line numbers (the comment click targets) remain visible and clickable. */
  .gutter {
    position: sticky;
    z-index: 1;
    width: var(--gutter-w);
    min-width: var(--gutter-w);
    max-width: var(--gutter-w);
    box-sizing: border-box;
    white-space: nowrap;
    padding: 0 8px;
    text-align: right;
    color: #8b949e;
    user-select: none;
    cursor: pointer;
    background: #fafbfc;
  }

  .old-gutter {
    left: 0;
  }

  .new-gutter {
    left: var(--gutter-w);
    /* Separator drawn as an inset shadow instead of a border: with
       border-collapse, collapsed borders don't travel with sticky cells. */
    box-shadow: inset -1px 0 0 #e2e2e2;
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

  /* Sticky cells need opaque backgrounds matching their row tint so the
     scrolled code doesn't bleed through underneath them. */
  .line-add .gutter {
    background: #ccffd8;
  }

  .line-remove .gutter {
    background: #ffd7d5;
  }

  .gutter:hover {
    background: #eaeef2;
  }

  .comment-row td {
    padding: 0;
    border: none;
  }

  /* Pins inline comment blocks and the comment editor to the visible
     scrollport: sticky so they don't ride along with horizontal code
     scroll, 100cqw (width of .hunk-body, the container) so they never
     extend past the visible area even though the row spans the full
     (possibly much wider) table. */
  .inline-anchor {
    position: sticky;
    left: 0;
    max-width: 100cqw;
    box-sizing: border-box;
  }

  .comment-block {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 10px;
    padding: 8px 12px;
    margin: 2px 8px;
    background: #fff8c5;
    border: 1px solid #d4c76a;
    border-radius: 4px;
    font-family: ui-sans-serif, system-ui, sans-serif;
    font-size: 13px;
    color: #333;
    white-space: pre-wrap;
  }

  .comment-body {
    flex: 1;
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

  .comment-editor-row td {
    padding: 0;
    border: none;
  }

  /* Horizontal inset lives on the anchor (inside its border-box width)
     rather than on the td, so the anchor's natural position starts at the
     scrollport's left edge and 100cqw spans exactly the visible width. */
  .comment-editor-row .inline-anchor {
    padding: 4px 8px;
  }
</style>
