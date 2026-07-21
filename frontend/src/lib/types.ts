// Mirrors the server JSON exactly (see src/session.rs, src/gitdiff.rs,
// src/model.rs, src/mapping.rs). Field names are snake_case to match the
// wire format verbatim.

export type Side = 'old' | 'new'
export type Risk = 'high' | 'medium' | 'low'
export type Verdict = 'approve' | 'request-changes'
export type LineKind = 'context' | 'add' | 'remove'

/** Line-ending form of one diff line — the display content has its newline
 * stripped, so this is the only way to tell an LF line from a CRLF one (or
 * from a final line with no trailing newline at all). */
export type Eol = 'lf' | 'crlf' | 'none'

export interface DiffLine {
  kind: LineKind
  content: string
  eol: Eol
  old_no: number | null
  new_no: number | null
}

export interface Hunk {
  old_start: number
  old_count: number
  new_start: number
  new_count: number
  section: string
  lines: DiffLine[]
}

export type ChangeKind = 'added' | 'deleted' | 'modified' | 'renamed' | 'copied'
export type ContentKind = 'text' | 'binary' | 'non-utf8' | 'too-large'

/** What kind of filesystem object a diff side is, derived from its git
 * mode. Separate from ContentKind because a symlink or gitlink can render
 * "text" content while being a different kind of object entirely. */
export type FileType = 'regular' | 'executable' | 'symlink' | 'gitlink'

export interface FileDiff {
  old_path: string | null
  new_path: string | null
  change_kind: ChangeKind
  content_kind: ContentKind
  old_mode: string | null
  new_mode: string | null
  old_type: FileType | null
  new_type: FileType | null
  old_oid: string | null
  new_oid: string | null
  old_size: number | null
  new_size: number | null
  /** Either side is a Git LFS pointer blob — the diff shows the pointer,
   * not the actual content. */
  lfs_pointer: boolean
  hunks: Hunk[]
}

export interface HunkRef {
  file: number
  hunk: number | null
}

export interface ConcernView {
  id: string
  title: string
  description: string | null
  risk: Risk | null
  unmapped: boolean
  hunks: HunkRef[]
}

export interface Comment {
  path: string
  side: Side
  line: number
  body: string
}

/** A changed line no concern claimed — surfaced via the synthetic
 * `_unmapped` concern. `side`/`line` follow the same old/new convention as
 * DiffLine: 'old' with old_no for removes, 'new' with new_no for adds. */
export interface UnmappedLine {
  file: number
  side: Side
  line: number
}

export interface ConcernDraft {
  verdict: Verdict | null
  comments: Comment[]
}

export interface Draft {
  concerns: Record<string, ConcernDraft>
  general_comments: string[]
  acknowledged_opaque: number[]
}

/** A structured warning surfaced to the reviewer (see model.rs). `path` /
 * `concern_id` are present only when the warning is scoped to one. */
export interface Warning {
  code: string
  severity: 'info' | 'warning'
  message: string
  path?: string
  concern_id?: string
}

/** Server-side validation limits echoed to the client so the UI can apply
 * the same bounds to its inputs (see session.rs / server.rs). */
export interface Limits {
  max_comments: number
  max_comment_chars: number
  max_draft_bytes: number
}

export interface Session {
  title: string
  summary: string | null
  files: FileDiff[]
  concerns: ConcernView[]
  warnings: Warning[]
  draft: Draft
  /** Current draft revision — PUT /draft must echo it back. */
  draft_revision: number
  limits: Limits
  /** Null while the review is open; otherwise how it ended, so the UI can
   * show the right terminal screen instead of calling every ending
   * "submitted". */
  finished: FinishedKind | null
  unmapped_lines: UnmappedLine[]
}

/** How a finished session ended. */
export type FinishedKind = 'submitted' | 'aborted' | 'timeout'

/** Result of `PUT /draft`. Success carries the new revision to echo on the
 * next save. A 409 is returned (not thrown) so callers can tell "another
 * tab saved" (`error: 'draft conflict'`, with the server's revision) apart
 * from "the session already ended" (`error: 'session finished'`, with how
 * it ended). */
export type SaveDraftResult =
  | { ok: true; revision: number }
  | {
      ok: false
      error: string
      current_revision?: number
      finished?: FinishedKind
      details?: string[]
    }
