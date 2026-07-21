// Pure helpers for line-ending display in HunkView — kept side-effect-free
// so the badge conditions are unit-testable without mounting the component.

import type { DiffLine } from './types'

/** CRLF lines get a small badge so an add/remove pair that differs only in
 * line endings is visually distinguishable — plain LF stays unmarked as the
 * unremarkable default. */
export function showCrlfBadge(line: DiffLine): boolean {
  return line.eol === 'crlf'
}

/** Lines with no trailing newline get the unified-diff convention marker
 * ("\ No newline at end of file") rendered right after them. */
export function showNoNewlineMarker(line: DiffLine): boolean {
  return line.eol === 'none'
}

/** The marker text itself, matching the unified diff convention. */
export const NO_NEWLINE_MARKER = '\\ No newline at end of file'
