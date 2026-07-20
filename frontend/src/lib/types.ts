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

export type FileStatus = 'modified' | 'added' | 'deleted' | 'renamed' | 'binary'

export interface FileDiff {
  old_path: string | null
  new_path: string | null
  status: FileStatus
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

export interface ConcernDraft {
  verdict: Verdict | null
  comments: Comment[]
}

export interface Draft {
  concerns: Record<string, ConcernDraft>
  general_comments: string[]
}

export interface Session {
  title: string
  summary: string | null
  files: FileDiff[]
  concerns: ConcernView[]
  warnings: string[]
  draft: Draft
  submitted: boolean
}
