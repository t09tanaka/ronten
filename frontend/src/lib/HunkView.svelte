<script lang="ts">
  import { rs } from './state.svelte'
  import CommentEditor from './CommentEditor.svelte'
  import { newTarget, oldTarget, type CommentLineInfo } from './anchors'
  import { NO_NEWLINE_MARKER, showCrlfBadge, showNoNewlineMarker } from './eol'
  import { highlightLine, langForPath } from './highlight'
  import { revealControlChars, revealInvisibles } from './invisibles'
  import type { Comment, ConcernView, DiffLine, FileDiff, Hunk, HunkRef } from './types'

  const COLLAPSE_THRESHOLD = 200

  interface Props {
    file: FileDiff
    hunk: Hunk
    hunkRef: HunkRef
    concernId: string
    oncommentline: (info: CommentLineInfo) => void
  }

  let { file, hunk, hunkRef, concernId, oncommentline }: Props = $props()

  const lang = $derived(langForPath(file.new_path ?? file.old_path))
  const comments = $derived(rs.draft.concerns[concernId]?.comments ?? [])

  function commentsFor(target: CommentLineInfo | null): Comment[] {
    if (!target) return []
    return comments.filter(
      (c) => c.path === target.path && c.side === target.side && c.line === target.line,
    )
  }

  function isPending(target: CommentLineInfo | null): boolean {
    if (!target) return false
    const p = rs.pendingCommentTarget
    return p != null && p.path === target.path && p.side === target.side && p.line === target.line
  }

  function deleteComment(comment: Comment): void {
    const all = rs.draft.concerns[concernId]?.comments
    if (!all) return
    const idx = all.indexOf(comment)
    if (idx >= 0) rs.removeComment(concernId, idx)
  }

  // Add lines only exist on the new side, remove lines only on the old
  // side — context lines are never unclaimed, since only add/remove lines
  // are changes a concern could have claimed.
  function isLineUnmapped(line: DiffLine): boolean {
    if (line.kind === 'add') return rs.isUnmappedLine(hunkRef.file, 'new', line.new_no)
    if (line.kind === 'remove') return rs.isUnmappedLine(hunkRef.file, 'old', line.old_no)
    return false
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
      <span class="hunk-section">{revealControlChars(hunk.section)}</span>
    {/if}
    {#if otherOwners.length > 0}
      <span class="shared-badge">
        shared with
        {#each otherOwners as owner, i (owner.id)}
          <button type="button" class="owner-link" onclick={() => jumpTo(owner.id)}
            >{revealControlChars(owner.title)}</button
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
            {@const oldT = oldTarget(file, line)}
            {@const newT = newTarget(file, line)}
            {@const pendingT = isPending(oldT) ? oldT : isPending(newT) ? newT : null}
            <tr class="line line-{line.kind}" class:line-unmapped={isLineUnmapped(line)}>
              <td class="gutter old-gutter">
                {#if oldT}
                  <button
                    type="button"
                    class="gutter-btn"
                    aria-label="Add a comment on old line {oldT.line}"
                    onclick={() => oncommentline(oldT)}>{line.old_no}</button
                  >
                {/if}
              </td>
              <td class="gutter new-gutter">
                {#if newT}
                  <button
                    type="button"
                    class="gutter-btn"
                    aria-label="Add a comment on new line {newT.line}"
                    onclick={() => oncommentline(newT)}>{line.new_no}</button
                  >
                {/if}
              </td>
              <!-- highlightLine always returns HTML-escaped markup (hljs
                   escapes its input; the fallback path escapes explicitly),
                   so agent-supplied diff content cannot inject HTML here.
                   revealInvisibles runs first, on the plain content, so its
                   ⟨U+XXXX⟩ markers also flow through that escaping. -->
              <td class="content"
                >{@html highlightLine(revealInvisibles(line.content), lang)}{#if showCrlfBadge(line)}<span
                    class="eol-badge"
                    title="Line ends with CRLF (␍␊)">CRLF</span
                  >{/if}</td
              >
            </tr>
            {#if showNoNewlineMarker(line)}
              <!-- Unified-diff convention: flag the absence of a trailing
                   newline right after the affected line, so an add/remove
                   pair differing only in the final newline reads as a real
                   change. Empty sticky gutters keep column alignment. -->
              <tr class="no-eol-row">
                <td class="gutter old-gutter"></td>
                <td class="gutter new-gutter"></td>
                <td class="content no-eol-marker">{NO_NEWLINE_MARKER}</td>
              </tr>
            {/if}
            {#each [...commentsFor(oldT), ...commentsFor(newT)] as comment (comment)}
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
            {#if pendingT}
              <tr class="comment-editor-row">
                <td colspan="3">
                  <div class="inline-anchor">
                    <CommentEditor
                      {concernId}
                      path={pendingT.path}
                      side={pendingT.side}
                      line={pendingT.line}
                    />
                  </div>
                </td>
              </tr>
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
    border: 1px solid var(--c-rule);
    border-top: none;
  }

  .hunk-header {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 4px 10px;
    background: var(--c-panel);
    color: var(--c-ink-2);
    font-family: var(--font-mono);
    font-size: 12px;
    border-bottom: 1px solid var(--c-rule);
    flex-wrap: wrap;
  }

  .hunk-range {
    color: var(--c-ink-2);
  }

  .hunk-section {
    color: var(--c-ink-2);
    /* Trojan Source defense: isolate the agent-supplied section text from
       the surrounding UI so bidi control characters in it can't reorder
       neighboring elements (their codepoints are also revealed as
       ⟨U+XXXX⟩ tokens by revealControlChars). */
    unicode-bidi: isolate;
  }

  .shared-badge {
    margin-left: auto;
    font-size: 11px;
    color: var(--c-odo);
    display: flex;
    align-items: center;
    gap: 3px;
  }

  .owner-link {
    background: none;
    border: none;
    color: var(--c-ai);
    cursor: pointer;
    padding: 0;
    font-size: 11px;
    text-decoration: underline;
    font-family: inherit;
    unicode-bidi: isolate;
  }

  .collapse-toggle {
    display: block;
    width: 100%;
    text-align: left;
    padding: 8px 10px;
    background: var(--c-panel);
    border: none;
    border-top: 1px solid var(--c-rule);
    color: var(--c-ink-2);
    cursor: pointer;
    font-family: var(--font-mono);
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
    font-family: var(--font-mono);
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
    padding: 0;
    text-align: right;
    color: var(--c-ink-2);
    user-select: none;
    background: var(--c-gutter);
  }

  /* The whole gutter cell is one native button (keyboard-focusable,
     Enter/Space activate) styled to look like the plain line-number
     gutter: transparent background, full-cell click area, right-aligned
     number. */
  .gutter-btn {
    display: block;
    width: 100%;
    box-sizing: border-box;
    padding: 0 8px;
    margin: 0;
    border: none;
    background: transparent;
    color: inherit;
    font: inherit;
    text-align: right;
    cursor: pointer;
  }

  .gutter-btn:hover {
    background: #edebe1;
  }

  .gutter-btn:focus-visible {
    outline: 2px solid var(--c-ai);
    outline-offset: -2px;
  }

  .old-gutter {
    left: 0;
  }

  .new-gutter {
    left: var(--gutter-w);
    /* Separator drawn as an inset shadow instead of a border: with
       border-collapse, collapsed borders don't travel with sticky cells. */
    box-shadow: inset -1px 0 0 var(--c-rule);
  }

  .content {
    padding: 0 10px;
    white-space: pre;
    /* Trojan Source defense: pin display order to logical order so bidi
       control characters in the diff can't reorder how code renders (their
       codepoints are also revealed as ⟨U+XXXX⟩ tokens by revealInvisibles). */
    unicode-bidi: isolate;
    direction: ltr;
  }

  /* Faint marker, not a highlight: it must be findable when hunting an
     EOL-only change without shouting on every line of a CRLF file. */
  .eol-badge {
    margin-left: 8px;
    padding: 0 4px;
    border-radius: 2px;
    background: var(--c-neutral-tint);
    color: var(--c-ink-3);
    font-size: 9.5px;
    user-select: none;
  }

  .no-eol-marker {
    color: var(--c-ink-3);
  }

  .line-add {
    background: var(--c-matsuba-tint);
  }

  .line-remove {
    background: var(--c-shu-tint);
  }

  /* Sticky cells need opaque backgrounds matching their row tint so the
     scrolled code doesn't bleed through underneath them. */
  .line-add .gutter {
    background: var(--c-matsuba-tint-2);
  }

  .line-remove .gutter {
    background: var(--c-shu-tint-2);
  }

  /* A changed line no concern claimed — flagged for extra care while
     viewing the synthetic `_unmapped` concern. Overrides the add/remove
     tint (higher-specificity compound selectors) with the ōdo warning
     family, so it reads as a distinct signal rather than a shade of the
     usual add/remove coloring. The left border lives on .content (a
     regular, non-sticky td) rather than the row itself, matching the
     .new-gutter separator's box-shadow pattern below — box-shadow on a
     table row does not render reliably across browsers. */
  .line.line-unmapped {
    background: var(--c-odo-tint);
  }

  .line.line-unmapped .gutter {
    background: var(--c-odo-tint-2);
  }

  .line.line-unmapped .content {
    box-shadow: inset 3px 0 0 var(--c-odo);
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
    background: var(--c-ai-tint);
    border: 1px solid var(--c-ai-border);
    border-radius: 4px;
    font-family: var(--font-body);
    font-size: 13px;
    color: var(--c-ink);
    white-space: pre-wrap;
  }

  .comment-body {
    flex: 1;
  }

  .comment-delete {
    flex-shrink: 0;
    border: none;
    background: none;
    color: var(--c-ink-2);
    cursor: pointer;
    font-size: 14px;
    line-height: 1;
    padding: 0 2px;
  }

  .comment-delete:hover {
    color: var(--c-shu);
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
