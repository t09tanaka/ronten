// Pure helpers for the opaque-change detail card and file-header badges in
// DiffView — kept side-effect-free so the display logic is unit-testable
// without mounting the component.

import type { AckReason, ContentKind, FileDiff } from './types'

/** "mode 100644 → 100755" when both sides are present and differ. */
export function modeChangeBadge(f: FileDiff): string | null {
  if (f.old_mode == null || f.new_mode == null || f.old_mode === f.new_mode) return null
  return `mode ${f.old_mode} → ${f.new_mode}`
}

/** "added symlink 120000" / "added executable 100755" / "deleted symlink
 * 120000" — a one-sided file (added/deleted) whose mode carries meaning
 * beyond the ordinary "added"/"deleted" badge already shown. `null` for a
 * plain regular file or a two-sided change (see `modeChangeBadge`). */
export function oneSidedModeBadge(f: FileDiff): string | null {
  if (f.change_kind === 'added' && f.new_mode != null) {
    if (f.new_type === 'symlink') return `added symlink ${f.new_mode}`
    if (f.new_type === 'executable') return `added executable ${f.new_mode}`
  }
  if (f.change_kind === 'deleted' && f.old_mode != null && f.old_type === 'symlink') {
    return `deleted symlink ${f.old_mode}`
  }
  return null
}

/** "regular → symlink" when both sides are present and differ. */
export function typeChangeBadge(f: FileDiff): string | null {
  if (f.old_type == null || f.new_type == null || f.old_type === f.new_type) return null
  return `${f.old_type} → ${f.new_type}`
}

/** Human-readable label for one `AckReason`, used both in the ack card and
 * in `fileNotices` below. Keep in sync with the variant list in
 * `types.ts`/`gitdiff.rs`. */
const ACK_REASON_LABELS: Record<AckReason, string> = {
  'opaque-content': 'Content not rendered — the summary below is all that is shown',
  'gitlink-changed': 'Submodule pointer change — nested diff not shown',
  'mode-changed': 'File mode changed',
  'added-symlink': 'New symlink added',
  'deleted-symlink': 'Symlink deleted',
  'added-executable': 'New executable file added',
  'regular-to-symlink': 'Regular file became a symlink',
  'lfs-pointer': 'Git LFS pointer — actual content not shown',
  'submodule-pointer': 'Submodule reference added or removed — nested diff not shown',
}

/** Human-readable label for every reason this file requires an ack, in
 * server order. Drives the ack card's explanatory text. */
export function ackReasonLabels(f: FileDiff): string[] {
  return f.ack_reasons.map((r) => ACK_REASON_LABELS[r])
}

/** Always-visible notices for changes whose real content is not the text
 * shown in the diff body (submodule pointers, LFS pointers) — derived from
 * the server's `ack_reasons`, not recomputed. */
export function fileNotices(f: FileDiff): string[] {
  const noticeReasons: AckReason[] = ['gitlink-changed', 'submodule-pointer', 'lfs-pointer']
  return f.ack_reasons.filter((r) => noticeReasons.includes(r)).map((r) => ACK_REASON_LABELS[r])
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
