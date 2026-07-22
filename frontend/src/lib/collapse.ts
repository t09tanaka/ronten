// Pure decision helpers for bounding how much hunk DOM mounts at once.
//
// Two independent rules cooperate rather than compete: a single oversized
// hunk collapses on its own (HunkView's own COLLAPSE_THRESHOLD, unchanged),
// while THIS threshold catches the opposite shape — a concern owning many
// small hunks, each individually under that per-hunk threshold, whose
// combined rendered-line count would still build an unbounded amount of DOM
// (e.g. 1,000 hunks x 20 lines = 20,000 lines, no single hunk of which would
// ever trip the 200-line rule). Both rules only ever widen the initial
// collapsed set (logical OR) — never toggle each other off — so a concern
// with one huge hunk and many small ones collapses the huge one for its own
// reason and, separately, forces the small ones collapsed too once the
// total crosses this threshold.
export const CONCERN_TOTAL_LINES_COLLAPSE_THRESHOLD = 1000

/** Whether every hunk of the currently selected concern should start
 * collapsed because the concern's total rendered line count (summed across
 * all its hunks) exceeds the threshold. `totalLines` is the caller-computed
 * sum of `hunk.lines.length` over every hunk the concern owns. */
export function shouldCollapseAllHunks(totalLines: number): boolean {
  return totalLines > CONCERN_TOTAL_LINES_COLLAPSE_THRESHOLD
}
