// Pure helpers for the opaque-change detail card in DiffView — kept
// side-effect-free so the display logic is unit-testable without mounting
// the component.

import type { ContentKind, FileDiff } from './types'

export function contentNote(kind: ContentKind): string {
  switch (kind) {
    case 'text':
      return ''
    case 'binary':
      return 'Binary file changed (content not displayed)'
    case 'non-utf8':
      return 'Non-UTF-8 file changed (content not displayed)'
    case 'too-large':
      return 'File too large to display (content not displayed)'
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
