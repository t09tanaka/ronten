// Pure helpers for the `_unmapped` concern's line highlighting: which
// changed lines no concern claimed. Kept side-effect-free (no Svelte
// runes) so the keying/lookup logic is unit-testable without mounting a
// component — the reactive wrapper lives in state.svelte.ts.

import type { Side, UnmappedLine } from './types'

/** Stable key for a (file, side, line) triple used both when building the
 * set and when looking a line up. */
export function unmappedKey(file: number, side: Side, line: number): string {
  return `${file}:${side}:${line}`
}

/** Builds the O(1)-lookup set from the server's unmapped_lines list. */
export function buildUnmappedSet(lines: UnmappedLine[]): Set<string> {
  return new Set(lines.map((l) => unmappedKey(l.file, l.side, l.line)))
}

/** Whether (file, side, line) is one of the unclaimed changed lines.
 * `line` is nullable because context lines (and the missing side of an
 * add/remove line) have no line number on one side — those never
 * highlight. */
export function isUnmappedInSet(
  set: Set<string>,
  file: number,
  side: Side,
  line: number | null,
): boolean {
  if (line == null) return false
  return set.has(unmappedKey(file, side, line))
}
