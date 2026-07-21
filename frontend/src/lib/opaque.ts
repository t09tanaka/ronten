// Pure helpers for the opaque-change detail card and file-header badges in
// DiffView — kept side-effect-free so the display logic is unit-testable
// without mounting the component.

import type { ContentKind, FileDiff } from './types'

/** Mirror of the server's `FileDiff::requires_ack()` (gitdiff.rs): opaque
 * content, a gitlink whose pointer actually moved (a same-oid pure rename
 * moves nothing), or a mode change with both sides present. Submit
 * validation rejects drafts missing these acks, so this must stay in
 * lockstep with the server condition. */
export function requiresAck(f: FileDiff): boolean {
  if (f.content_kind !== 'text') return true
  const gitlinkInvolved = f.old_type === 'gitlink' || f.new_type === 'gitlink'
  if (gitlinkInvolved && f.old_oid !== f.new_oid) return true
  return f.old_mode != null && f.new_mode != null && f.old_mode !== f.new_mode
}

/** "mode 100644 → 100755" when both sides are present and differ. */
export function modeChangeBadge(f: FileDiff): string | null {
  if (f.old_mode == null || f.new_mode == null || f.old_mode === f.new_mode) return null
  return `mode ${f.old_mode} → ${f.new_mode}`
}

/** "regular → symlink" when both sides are present and differ. */
export function typeChangeBadge(f: FileDiff): string | null {
  if (f.old_type == null || f.new_type == null || f.old_type === f.new_type) return null
  return `${f.old_type} → ${f.new_type}`
}

/** Always-visible notices for changes whose real content is not the text
 * shown in the diff body (submodule pointers, LFS pointers). */
export function fileNotices(f: FileDiff): string[] {
  const notices: string[] = []
  const gitlinkInvolved = f.old_type === 'gitlink' || f.new_type === 'gitlink'
  if (gitlinkInvolved && f.old_oid !== f.new_oid) {
    notices.push('Submodule pointer change — nested diff not shown')
  }
  if (f.lfs_pointer) {
    notices.push('Git LFS pointer — actual content not shown')
  }
  return notices
}

export function contentNote(kind: ContentKind): string {
  switch (kind) {
    case 'text':
      return ''
    case 'binary':
      return 'Binary file changed — contents not displayed'
    case 'non-utf8':
      return 'Non-UTF-8 file changed — contents not displayed'
    case 'too-large':
      return 'File changed — contents omitted because a display limit was exceeded'
    default: {
      const exhaustive: never = kind
      return exhaustive
    }
  }
}

export interface OpaqueDetailRow {
  label: string
  value: string
}

/** mode/oid/size を "old → new"（片側は "—"）で並べる。null 同士の行は出さない。 */
export function opaqueDetails(f: FileDiff): OpaqueDetailRow[] {
  const rows: OpaqueDetailRow[] = []
  const pair = (a: string | null, b: string | null): string => `${a ?? '—'} → ${b ?? '—'}`
  if (f.old_mode != null || f.new_mode != null) rows.push({ label: 'mode', value: pair(f.old_mode, f.new_mode) })
  if (f.old_oid != null || f.new_oid != null)
    rows.push({ label: 'oid', value: pair(f.old_oid && f.old_oid.slice(0, 12), f.new_oid && f.new_oid.slice(0, 12)) })
  if (f.old_size != null || f.new_size != null)
    rows.push({
      label: 'size',
      value: pair(
        f.old_size != null ? `${f.old_size.toLocaleString()} B` : null,
        f.new_size != null ? `${f.new_size.toLocaleString()} B` : null,
      ),
    })
  return rows
}
