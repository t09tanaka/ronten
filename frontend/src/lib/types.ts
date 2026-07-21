// Mirrors the server JSON exactly (see src/session.rs, src/gitdiff.rs,
// src/model.rs, src/mapping.rs). Field names are snake_case to match the
// wire format verbatim.

export type Side = 'old' | 'new'
export type Risk = 'high' | 'medium' | 'low'
export type Verdict = 'approve' | 'request-changes'
export type LineKind = 'context' | 'add' | 'remove'

export interface DiffLine {
  kind: LineKind
  content: string
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

export interface FileDiff {
  old_path: string | null
  new_path: string | null
  change_kind: ChangeKind
  content_kind: ContentKind
  old_mode: string | null
  new_mode: string | null
  old_oid: string | null
  new_oid: string | null
  old_size: number | null
  new_size: number | null
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

export interface Session {
  title: string
  summary: string | null
  files: FileDiff[]
  concerns: ConcernView[]
  warnings: string[]
  draft: Draft
  submitted: boolean
  unmapped_lines: UnmappedLine[]
}
