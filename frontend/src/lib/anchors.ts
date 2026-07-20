// Pure helpers that resolve a diff line to the comment anchor the server
// expects. The server maps side:'old' comments against the file's old_path
// and side:'new' comments against new_path, so each gutter must anchor to
// its own side's path — using new_path for an old-side comment breaks on
// renamed files.

import type { DiffLine, FileDiff, Side } from './types'

export interface CommentLineInfo {
  path: string
  side: Side
  line: number
}

/** Anchor for the old (left) gutter, or null when the line has no old side. */
export function oldTarget(file: FileDiff, line: DiffLine): CommentLineInfo | null {
  if (file.old_path == null || line.old_no == null) return null
  return { path: file.old_path, side: 'old', line: line.old_no }
}

/** Anchor for the new (right) gutter, or null when the line has no new side. */
export function newTarget(file: FileDiff, line: DiffLine): CommentLineInfo | null {
  if (file.new_path == null || line.new_no == null) return null
  return { path: file.new_path, side: 'new', line: line.new_no }
}
