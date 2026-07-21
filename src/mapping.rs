//! Maps git diff hunks to agent-declared concerns.
//!
//! A concern claims individual changed (`Add`/`Remove`) lines, not whole
//! hunks by range intersection — a location only claims a line whose own
//! line number falls in its range, never a hunk's context lines. The same
//! changed line may be claimed by multiple concerns (overlap is allowed).
//! A hunk is displayed under a concern once it claims at least one changed
//! line inside it. Changed lines (and hunk-less files) claimed by no
//! concern are reported in `Mapping.unmapped_lines` / `Mapping.unmapped`.

use crate::gitdiff::{FileDiff, LineKind};
use crate::model::{ConcernsInput, Side, SUPPORTED_VERSION};
use serde::Serialize;

/// Reserved concern id for the synthetic "everything nobody claimed" bucket
/// used by callers when rendering the unmapped hunks; `validate_concerns`
/// rejects any real concern from using it.
pub const UNMAPPED_ID: &str = "_unmapped";

/// A reference to a single hunk within a file, or (when `hunk` is `None`)
/// the whole of a hunk-less file (binary diff or pure rename).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct HunkRef {
    pub file: usize,
    pub hunk: Option<usize>,
}

/// A concern together with the hunks its locations resolved to.
#[derive(Debug, Clone, Serialize)]
pub struct MappedConcern {
    pub id: String,
    pub hunks: Vec<HunkRef>,
}

/// A single unclaimed changed line (`_unmapped` highlight target).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct UnmappedLine {
    pub file: usize,
    /// `Add` lines are `Side::New`, `Remove` lines are `Side::Old`.
    pub side: Side,
    /// The line number on `side` (`new_no` for `Add`, `old_no` for `Remove`).
    pub line: u32,
}

/// The result of resolving every concern's locations against a diff.
#[derive(Debug)]
pub struct Mapping {
    pub concerns: Vec<MappedConcern>,
    pub unmapped: Vec<HunkRef>,
    pub unmapped_lines: Vec<UnmappedLine>,
    pub warnings: Vec<String>,
}

/// True when `id` matches `^[A-Za-z0-9][A-Za-z0-9._-]*$` (hand-rolled to
/// avoid pulling in a regex crate).
fn valid_id_pattern(id: &str) -> bool {
    let mut chars = id.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphanumeric() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Validates the concerns list itself (not against a diff): rejects an
/// unsupported contract version, an empty or oversized list, duplicate ids,
/// malformed ids (including the reserved `_unmapped`), over-long text
/// fields, and invalid location ranges.
pub fn validate_concerns(input: &ConcernsInput) -> Result<(), String> {
    if input.version != SUPPORTED_VERSION {
        return Err(format!(
            "unsupported version {} (this ronten supports version {SUPPORTED_VERSION})",
            input.version
        ));
    }
    if input.concerns.is_empty() {
        return Err("concerns list must not be empty".to_string());
    }
    if input.concerns.len() > 200 {
        return Err(format!(
            "too many concerns: {} (maximum 200)",
            input.concerns.len()
        ));
    }
    if let Some(summary) = &input.summary {
        if summary.chars().count() > 2_000 {
            return Err("summary exceeds 2000 characters".to_string());
        }
    }
    let mut seen = std::collections::HashSet::new();
    for c in &input.concerns {
        if c.id == UNMAPPED_ID {
            return Err(format!(
                "concern id \"{UNMAPPED_ID}\" is reserved and cannot be used"
            ));
        }
        if c.id.is_empty() || c.id.chars().count() > 64 || !valid_id_pattern(&c.id) {
            return Err(format!(
                "invalid concern id {:?}: must be 1-64 characters matching ^[A-Za-z0-9][A-Za-z0-9._-]*$",
                c.id
            ));
        }
        if !seen.insert(c.id.as_str()) {
            return Err(format!("duplicate concern id: {}", c.id));
        }
        if c.title.trim().is_empty() {
            return Err(format!("concern {:?}: title must not be blank", c.id));
        }
        if c.title.chars().count() > 200 {
            return Err(format!("concern {:?}: title exceeds 200 characters", c.id));
        }
        if let Some(description) = &c.description {
            if description.chars().count() > 20_000 {
                return Err(format!(
                    "concern {:?}: description exceeds 20000 characters",
                    c.id
                ));
            }
        }
        if c.locations.len() > 200 {
            return Err(format!(
                "concern {:?}: too many locations: {} (maximum 200)",
                c.id,
                c.locations.len()
            ));
        }
        for loc in &c.locations {
            if loc.start == Some(0) || loc.end == Some(0) {
                return Err(format!(
                    "concern {:?}: location {}: line numbers are 1-based (0 is invalid)",
                    c.id, loc.path
                ));
            }
            if let (Some(start), Some(end)) = (loc.start, loc.end) {
                if start > end {
                    return Err(format!(
                        "concern {:?}: location {}: start {start} is greater than end {end}",
                        c.id, loc.path
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Sort key for `UnmappedLine.side` in `(file, side, line)` order (`Old`
/// before `New`); `Side` itself has no `Ord` since nothing else needs one.
fn side_sort_key(side: Side) -> u8 {
    match side {
        Side::Old => 0,
        Side::New => 1,
    }
}

/// Resolves every concern's locations against the diff's individual changed
/// lines.
///
/// - Path matching is unchanged from whole-hunk mapping: `side: "old"`
///   matches against `file.old_path`; `"new"` or an unspecified side matches
///   against `file.new_path`.
/// - Missing `start`/`end` default to `1`/`u32::MAX` (i.e. the whole file).
/// - A location claims a changed line when the line's own number (not the
///   hunk's range) falls in `[start, end]`: `side: "new"` only considers
///   `Add` lines (by `new_no`); `"old"` only `Remove` lines (by `old_no`);
///   an unspecified side considers both. Context lines are never claimable,
///   so a range that only overlaps context claims nothing.
/// - Hunk-less files (binary/pure rename) are only claimable by whole-file
///   locations (no `start` and no `end`), as `HunkRef { hunk: None }`, same
///   as before.
/// - A concern's displayed `hunks` are the distinct `(file, hunk)` pairs
///   (plus any hunk-less files) it claimed at least one line/whole-file in,
///   sorted by `(file, hunk)`.
/// - A location that claims nothing (no changed line, no hunk-less file)
///   produces a warning, never an error.
/// - `unmapped_lines` lists every changed line no concern claimed, sorted by
///   `(file, side, line)`.
/// - `unmapped` lists hunk-less files no concern claimed whole, plus every
///   hunk that still contains at least one line in `unmapped_lines` — a
///   concern partially claiming a hunk no longer hides the rest of that
///   hunk's changes.
pub fn resolve_mapping(files: &[FileDiff], input: &ConcernsInput) -> Mapping {
    use std::collections::{BTreeSet, HashMap, HashSet};

    // Step 1: per-file changed-line lists, and path -> file-index maps so
    // the concern/location sweep below never linearly rescans `files`.
    let mut adds: Vec<Vec<(usize, u32)>> = vec![Vec::new(); files.len()];
    let mut removes: Vec<Vec<(usize, u32)>> = vec![Vec::new(); files.len()];
    let mut by_new_path: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut by_old_path: HashMap<&str, Vec<usize>> = HashMap::new();
    for (fi, file) in files.iter().enumerate() {
        if let Some(p) = file.new_path.as_deref() {
            by_new_path.entry(p).or_default().push(fi);
        }
        if let Some(p) = file.old_path.as_deref() {
            by_old_path.entry(p).or_default().push(fi);
        }
        for (hi, h) in file.hunks.iter().enumerate() {
            for line in &h.lines {
                match line.kind {
                    LineKind::Add => {
                        if let Some(no) = line.new_no {
                            adds[fi].push((hi, no));
                        }
                    }
                    LineKind::Remove => {
                        if let Some(no) = line.old_no {
                            removes[fi].push((hi, no));
                        }
                    }
                    LineKind::Context => {}
                }
            }
        }
    }

    // Step 2: claim sets.
    let mut warnings = Vec::new();
    let mut claimed_adds: HashSet<(usize, u32)> = HashSet::new();
    let mut claimed_removes: HashSet<(usize, u32)> = HashSet::new();
    let mut claimed_hunkless: HashSet<usize> = HashSet::new();
    let mut concerns = Vec::new();

    // Step 3: each concern's each location.
    for c in &input.concerns {
        let mut covered_hunks: BTreeSet<(usize, Option<usize>)> = BTreeSet::new();
        for loc in &c.locations {
            let file_indices: &[usize] = match loc.side.unwrap_or(Side::New) {
                Side::New => by_new_path.get(loc.path.as_str()),
                Side::Old => by_old_path.get(loc.path.as_str()),
            }
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
            let want_adds = !matches!(loc.side, Some(Side::Old));
            let want_removes = !matches!(loc.side, Some(Side::New));
            let start = loc.start.unwrap_or(1);
            let end = loc.end.unwrap_or(u32::MAX);
            let mut matched = false;

            for &fi in file_indices {
                let file = &files[fi];
                if file.hunks.is_empty() && loc.start.is_none() && loc.end.is_none() {
                    covered_hunks.insert((fi, None));
                    claimed_hunkless.insert(fi);
                    matched = true;
                    continue;
                }
                if want_adds {
                    for &(hi, no) in &adds[fi] {
                        if no >= start && no <= end {
                            claimed_adds.insert((fi, no));
                            covered_hunks.insert((fi, Some(hi)));
                            matched = true;
                        }
                    }
                }
                if want_removes {
                    for &(hi, no) in &removes[fi] {
                        if no >= start && no <= end {
                            claimed_removes.insert((fi, no));
                            covered_hunks.insert((fi, Some(hi)));
                            matched = true;
                        }
                    }
                }
            }

            if !matched {
                let range = match (loc.start, loc.end) {
                    (Some(s), Some(e)) => format!(":{s}-{e}"),
                    (Some(s), None) => format!(":{s}-"),
                    (None, Some(e)) => format!(":-{e}"),
                    (None, None) => String::new(),
                };
                warnings.push(format!(
                    "location matched no changed lines: {}{}",
                    loc.path, range
                ));
            }
        }

        // Step 4: concern's display hunks, sorted (file, hunk).
        let hunks: Vec<HunkRef> = covered_hunks
            .into_iter()
            .map(|(file, hunk)| HunkRef { file, hunk })
            .collect();
        concerns.push(MappedConcern {
            id: c.id.clone(),
            hunks,
        });
    }

    // Step 5: every changed line no concern claimed, tracking which hunks
    // they live in as we go (needed for step 6).
    let mut unmapped_lines = Vec::new();
    let mut unmapped_hunks_set: HashSet<(usize, usize)> = HashSet::new();
    for fi in 0..files.len() {
        for &(hi, no) in &adds[fi] {
            if !claimed_adds.contains(&(fi, no)) {
                unmapped_lines.push(UnmappedLine {
                    file: fi,
                    side: Side::New,
                    line: no,
                });
                unmapped_hunks_set.insert((fi, hi));
            }
        }
        for &(hi, no) in &removes[fi] {
            if !claimed_removes.contains(&(fi, no)) {
                unmapped_lines.push(UnmappedLine {
                    file: fi,
                    side: Side::Old,
                    line: no,
                });
                unmapped_hunks_set.insert((fi, hi));
            }
        }
    }
    unmapped_lines.sort_by_key(|l| (l.file, side_sort_key(l.side), l.line));

    // Step 6: hunk-less files never whole-claimed, plus hunks with >=1
    // unmapped line, in (file, hunk) order.
    let mut unmapped = Vec::new();
    for (fi, file) in files.iter().enumerate() {
        if file.hunks.is_empty() {
            if !claimed_hunkless.contains(&fi) {
                unmapped.push(HunkRef {
                    file: fi,
                    hunk: None,
                });
            }
        } else {
            for hi in 0..file.hunks.len() {
                if unmapped_hunks_set.contains(&(fi, hi)) {
                    unmapped.push(HunkRef {
                        file: fi,
                        hunk: Some(hi),
                    });
                }
            }
        }
    }

    Mapping {
        concerns,
        unmapped,
        unmapped_lines,
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gitdiff::{ChangeKind, ContentKind, DiffLine, Hunk};
    use crate::model::{Concern, Location, Risk};

    /// Fabricates a `FileDiff` with `old_path == new_path == path` and one
    /// `Hunk` per `(old_start, old_count, new_start, new_count)` tuple. Each
    /// hunk's `lines` are auto-generated as fully changed (no context): a
    /// `Remove` for every line in the old range, an `Add` for every line in
    /// the new range — matching what a real diff hunk header describes.
    fn fd(path: &str, hunks: &[(u32, u32, u32, u32)]) -> FileDiff {
        FileDiff {
            old_path: Some(path.to_string()),
            new_path: Some(path.to_string()),
            change_kind: ChangeKind::Modified,
            content_kind: ContentKind::Text,
            old_mode: None,
            new_mode: None,
            old_oid: None,
            new_oid: None,
            old_size: None,
            new_size: None,
            hunks: hunks
                .iter()
                .map(|&(old_start, old_count, new_start, new_count)| {
                    let mut lines = Vec::new();
                    for i in 0..old_count {
                        lines.push(DiffLine {
                            kind: LineKind::Remove,
                            content: String::new(),
                            old_no: Some(old_start + i),
                            new_no: None,
                        });
                    }
                    for i in 0..new_count {
                        lines.push(DiffLine {
                            kind: LineKind::Add,
                            content: String::new(),
                            old_no: None,
                            new_no: Some(new_start + i),
                        });
                    }
                    Hunk {
                        old_start,
                        old_count,
                        new_start,
                        new_count,
                        section: String::new(),
                        lines,
                    }
                })
                .collect(),
        }
    }

    /// Fabricates a single-hunk `FileDiff` with explicit changed lines (from
    /// `changed`, each `(kind, old_no, new_no)`) plus context lines (each
    /// `n` becomes a `Context` line with `old_no == new_no == n`), combined
    /// and ordered by line number.
    fn fd_with_lines(
        path: &str,
        old_start: u32,
        old_count: u32,
        new_start: u32,
        new_count: u32,
        changed: &[(LineKind, Option<u32>, Option<u32>)],
        context_lines: &[u32],
    ) -> FileDiff {
        let mut lines: Vec<DiffLine> = changed
            .iter()
            .map(|&(kind, old_no, new_no)| DiffLine {
                kind,
                content: String::new(),
                old_no,
                new_no,
            })
            .collect();
        for &n in context_lines {
            lines.push(DiffLine {
                kind: LineKind::Context,
                content: String::new(),
                old_no: Some(n),
                new_no: Some(n),
            });
        }
        lines.sort_by_key(|l| l.new_no.or(l.old_no).unwrap_or(u32::MAX));
        FileDiff {
            old_path: Some(path.to_string()),
            new_path: Some(path.to_string()),
            change_kind: ChangeKind::Modified,
            content_kind: ContentKind::Text,
            old_mode: None,
            new_mode: None,
            old_oid: None,
            new_oid: None,
            old_size: None,
            new_size: None,
            hunks: vec![Hunk {
                old_start,
                old_count,
                new_start,
                new_count,
                section: String::new(),
                lines,
            }],
        }
    }

    fn concern(id: &str, locations: Vec<Location>) -> Concern {
        Concern {
            id: id.to_string(),
            title: "t".to_string(),
            description: None,
            risk: Risk::Medium,
            locations,
        }
    }

    fn loc(path: &str, side: Option<Side>, start: Option<u32>, end: Option<u32>) -> Location {
        Location {
            path: path.to_string(),
            side,
            start,
            end,
        }
    }

    fn input(concerns: Vec<Concern>) -> ConcernsInput {
        ConcernsInput {
            version: 1,
            summary: None,
            concerns,
        }
    }

    #[test]
    fn whole_file_location_claims_every_hunk() {
        let files = vec![fd("a.ts", &[(1, 2, 1, 2), (10, 3, 10, 3)])];
        let inp = input(vec![concern("c1", vec![loc("a.ts", None, None, None)])]);
        let mapping = resolve_mapping(&files, &inp);
        assert_eq!(
            mapping.concerns[0].hunks,
            vec![
                HunkRef {
                    file: 0,
                    hunk: Some(0)
                },
                HunkRef {
                    file: 0,
                    hunk: Some(1)
                },
            ]
        );
        assert!(mapping.warnings.is_empty());
    }

    #[test]
    fn range_intersection_boundaries() {
        // new range is [10, 19] (start 10, count 10)
        let files = vec![fd("a.ts", &[(10, 10, 10, 10)])];
        let inp = input(vec![concern(
            "c1",
            vec![
                loc("a.ts", None, Some(19), Some(30)),
                loc("a.ts", None, Some(20), None),
            ],
        )]);
        let mapping = resolve_mapping(&files, &inp);
        // first location (19-30) matches; second (20-) does not
        assert_eq!(
            mapping.concerns[0].hunks,
            vec![HunkRef {
                file: 0,
                hunk: Some(0)
            }]
        );
        assert_eq!(mapping.warnings.len(), 1);
        assert_eq!(
            mapping.warnings[0],
            "location matched no changed lines: a.ts:20-"
        );
    }

    #[test]
    fn old_side_matches_deletion_hunk_on_old_path_with_no_new_path() {
        let files = vec![FileDiff {
            old_path: Some("gone.txt".to_string()),
            new_path: None,
            change_kind: ChangeKind::Deleted,
            content_kind: ContentKind::Text,
            old_mode: None,
            new_mode: None,
            old_oid: None,
            new_oid: None,
            old_size: None,
            new_size: None,
            hunks: vec![Hunk {
                old_start: 1,
                old_count: 5,
                new_start: 0,
                new_count: 0,
                section: String::new(),
                // 5 removed lines, no added lines (pure deletion).
                lines: (1..=5)
                    .map(|n| DiffLine {
                        kind: LineKind::Remove,
                        content: String::new(),
                        old_no: Some(n),
                        new_no: None,
                    })
                    .collect(),
            }],
        }];
        let inp = input(vec![concern(
            "c1",
            vec![loc("gone.txt", Some(Side::Old), None, None)],
        )]);
        let mapping = resolve_mapping(&files, &inp);
        assert_eq!(
            mapping.concerns[0].hunks,
            vec![HunkRef {
                file: 0,
                hunk: Some(0)
            }]
        );
        assert!(mapping.warnings.is_empty());
    }

    #[test]
    fn hunk_intersecting_two_concerns_belongs_to_both() {
        let files = vec![fd("a.ts", &[(1, 10, 1, 10)])];
        let inp = input(vec![
            concern("c1", vec![loc("a.ts", None, None, None)]),
            concern("c2", vec![loc("a.ts", None, Some(5), Some(6))]),
        ]);
        let mapping = resolve_mapping(&files, &inp);
        let expect = vec![HunkRef {
            file: 0,
            hunk: Some(0),
        }];
        assert_eq!(mapping.concerns[0].hunks, expect);
        assert_eq!(mapping.concerns[1].hunks, expect);
    }

    #[test]
    fn unclaimed_hunks_land_in_unmapped_full_coverage_is_empty() {
        let files = vec![fd("a.ts", &[(1, 2, 1, 2), (10, 3, 10, 3)])];
        // Only the first hunk is claimed.
        let inp = input(vec![concern(
            "c1",
            vec![loc("a.ts", None, Some(1), Some(2))],
        )]);
        let mapping = resolve_mapping(&files, &inp);
        assert_eq!(
            mapping.unmapped,
            vec![HunkRef {
                file: 0,
                hunk: Some(1)
            }]
        );

        // Whole-file location claims both hunks -> nothing unmapped.
        let inp_full = input(vec![concern("c1", vec![loc("a.ts", None, None, None)])]);
        let mapping_full = resolve_mapping(&files, &inp_full);
        assert!(mapping_full.unmapped.is_empty());
    }

    #[test]
    fn location_matching_zero_hunks_warns_without_error() {
        let files = vec![fd("a.ts", &[(1, 2, 1, 2)])];
        let inp = input(vec![concern(
            "c1",
            vec![loc("a.ts", None, Some(5), Some(10))],
        )]);
        let mapping = resolve_mapping(&files, &inp);
        assert_eq!(mapping.concerns[0].hunks, Vec::new());
        assert_eq!(
            mapping.warnings,
            vec!["location matched no changed lines: a.ts:5-10".to_string()]
        );
    }

    #[test]
    fn hunk_less_binary_file_claimed_by_whole_file_location_else_unmapped() {
        let files = vec![
            FileDiff {
                old_path: Some("logo.png".to_string()),
                new_path: Some("logo.png".to_string()),
                change_kind: ChangeKind::Modified,
                content_kind: ContentKind::Binary,
                old_mode: None,
                new_mode: None,
                old_oid: None,
                new_oid: None,
                old_size: None,
                new_size: None,
                hunks: Vec::new(),
            },
            FileDiff {
                old_path: Some("other.png".to_string()),
                new_path: Some("other.png".to_string()),
                change_kind: ChangeKind::Modified,
                content_kind: ContentKind::Binary,
                old_mode: None,
                new_mode: None,
                old_oid: None,
                new_oid: None,
                old_size: None,
                new_size: None,
                hunks: Vec::new(),
            },
        ];
        let inp = input(vec![concern("c1", vec![loc("logo.png", None, None, None)])]);
        let mapping = resolve_mapping(&files, &inp);
        assert_eq!(
            mapping.concerns[0].hunks,
            vec![HunkRef {
                file: 0,
                hunk: None
            }]
        );
        assert_eq!(
            mapping.unmapped,
            vec![HunkRef {
                file: 1,
                hunk: None
            }]
        );
    }

    #[test]
    fn end_only_location_warning_keeps_the_end_bound() {
        // Hunk new range [100, 104]; an end-only location covering [1, 30]
        // matches nothing and must warn with the end bound preserved.
        let files = vec![fd("a.ts", &[(100, 5, 100, 5)])];
        let inp = input(vec![concern("c1", vec![loc("a.ts", None, None, Some(30))])]);
        let mapping = resolve_mapping(&files, &inp);
        assert_eq!(mapping.concerns[0].hunks, Vec::new());
        assert_eq!(
            mapping.warnings,
            vec!["location matched no changed lines: a.ts:-30".to_string()]
        );
    }

    #[test]
    fn two_locations_matching_same_hunk_dedupe_to_one_ref() {
        // Both locations overlap the single hunk's new range [1, 10];
        // the concern must list the hunk exactly once.
        let files = vec![fd("a.ts", &[(1, 10, 1, 10)])];
        let inp = input(vec![concern(
            "c1",
            vec![
                loc("a.ts", None, Some(1), Some(5)),
                loc("a.ts", None, Some(4), Some(8)),
            ],
        )]);
        let mapping = resolve_mapping(&files, &inp);
        assert_eq!(
            mapping.concerns[0].hunks,
            vec![HunkRef {
                file: 0,
                hunk: Some(0)
            }]
        );
        assert!(mapping.warnings.is_empty());
    }

    #[test]
    fn zero_new_count_hunk_has_no_add_lines_to_claim() {
        // Pure-deletion hunk: old side removes lines 5,6,7; new side has
        // new_count=0, i.e. no added lines exist at all. Under whole-hunk
        // range intersection this used to be tested via the *range's*
        // start/end collapsing when a count is 0; under changed-line
        // claiming that collapse no longer exists — there is simply nothing
        // on the `new` side to claim, and everything on the `old` side is a
        // real removed line (no boundary quirk to special-case).
        let files = vec![fd("a.ts", &[(5, 3, 5, 0)])];

        // side=New can never match: there are zero Add lines in this hunk.
        let new_side = input(vec![concern(
            "c1",
            vec![loc("a.ts", Some(Side::New), Some(5), Some(5))],
        )]);
        let mapping_new = resolve_mapping(&files, &new_side);
        assert_eq!(mapping_new.concerns[0].hunks, Vec::new());
        assert_eq!(
            mapping_new.warnings,
            vec!["location matched no changed lines: a.ts:5-5".to_string()]
        );

        // side=Old over the full removed range claims all three lines.
        let old_side = input(vec![concern(
            "c1",
            vec![loc("a.ts", Some(Side::Old), Some(5), Some(7))],
        )]);
        let mapping_old = resolve_mapping(&files, &old_side);
        assert_eq!(
            mapping_old.concerns[0].hunks,
            vec![HunkRef {
                file: 0,
                hunk: Some(0)
            }]
        );
        assert!(mapping_old.warnings.is_empty());
        assert!(mapping_old.unmapped.is_empty());
        assert!(mapping_old.unmapped_lines.is_empty());
    }

    #[test]
    fn context_only_intersection_does_not_claim() {
        // hunk: new 10..=16, but the only changed line is the Add at new
        // 13 — everything else (10,11,12,14,15,16) is context. A location
        // covering only the context (10-12) must claim nothing.
        let files = vec![fd_with_lines(
            "a.ts",
            10,
            7,
            10,
            7,
            &[(LineKind::Add, None, Some(13))],
            &[10, 11, 12, 14, 15, 16],
        )];
        let inp = input(vec![concern(
            "c1",
            vec![loc("a.ts", None, Some(10), Some(12))],
        )]);
        let mapping = resolve_mapping(&files, &inp);
        assert!(mapping.concerns[0].hunks.is_empty());
        assert_eq!(mapping.warnings.len(), 1);
        assert!(mapping.warnings[0].contains("matched no changed lines"));
        // The only changed line is unclaimed -> its hunk is unmapped, and
        // the line itself shows up in unmapped_lines.
        assert_eq!(
            mapping.unmapped,
            vec![HunkRef {
                file: 0,
                hunk: Some(0)
            }]
        );
        assert_eq!(
            mapping.unmapped_lines,
            vec![UnmappedLine {
                file: 0,
                side: Side::New,
                line: 13
            }]
        );
    }

    #[test]
    fn partially_claimed_hunk_reports_remaining_lines_unmapped() {
        // One hunk with two Add lines (new 13, new 15); the location only
        // claims 13. The hunk still shows up under the concern (it claimed
        // >=1 line), but 15 remains in unmapped_lines, and the hunk itself
        // still lands in `unmapped` because an unexplained change remains.
        let files = vec![fd_with_lines(
            "a.ts",
            13,
            3,
            13,
            3,
            &[
                (LineKind::Add, None, Some(13)),
                (LineKind::Add, None, Some(15)),
            ],
            &[14],
        )];
        let inp = input(vec![concern(
            "c1",
            vec![loc("a.ts", None, Some(13), Some(13))],
        )]);
        let mapping = resolve_mapping(&files, &inp);
        assert_eq!(
            mapping.concerns[0].hunks,
            vec![HunkRef {
                file: 0,
                hunk: Some(0)
            }]
        );
        assert_eq!(
            mapping.unmapped_lines,
            vec![UnmappedLine {
                file: 0,
                side: Side::New,
                line: 15
            }]
        );
        assert_eq!(
            mapping.unmapped,
            vec![HunkRef {
                file: 0,
                hunk: Some(0)
            }]
        );
    }

    #[test]
    fn unspecified_side_claims_both_adds_and_removes() {
        // A modification: old10 removed, new10 added. A side-unspecified
        // location on 10-10 must claim both.
        let files = vec![fd_with_lines(
            "a.ts",
            10,
            1,
            10,
            1,
            &[
                (LineKind::Remove, Some(10), None),
                (LineKind::Add, None, Some(10)),
            ],
            &[],
        )];
        let inp = input(vec![concern(
            "c1",
            vec![loc("a.ts", None, Some(10), Some(10))],
        )]);
        let mapping = resolve_mapping(&files, &inp);
        assert!(mapping.unmapped.is_empty());
        assert!(mapping.unmapped_lines.is_empty());
    }

    #[test]
    fn old_side_location_claims_only_removes() {
        let files = vec![fd_with_lines(
            "a.ts",
            10,
            1,
            10,
            1,
            &[
                (LineKind::Remove, Some(10), None),
                (LineKind::Add, None, Some(10)),
            ],
            &[],
        )];
        let inp = input(vec![concern(
            "c1",
            vec![loc("a.ts", Some(Side::Old), Some(10), Some(10))],
        )]);
        let mapping = resolve_mapping(&files, &inp);
        // The add at new10 is left unclaimed.
        assert_eq!(
            mapping.unmapped_lines,
            vec![UnmappedLine {
                file: 0,
                side: Side::New,
                line: 10
            }]
        );
    }

    #[test]
    fn validate_concerns_rejects_duplicates_reserved_id_and_empty() {
        let dup = input(vec![concern("c1", vec![]), concern("c1", vec![])]);
        assert!(validate_concerns(&dup).is_err());

        let reserved = input(vec![concern(UNMAPPED_ID, vec![])]);
        assert!(validate_concerns(&reserved).is_err());

        let empty = input(vec![]);
        assert!(validate_concerns(&empty).is_err());

        let valid = input(vec![concern("c1", vec![]), concern("c2", vec![])]);
        assert!(validate_concerns(&valid).is_ok());
    }

    #[test]
    fn validate_concerns_rejects_unsupported_version() {
        let mut inp = input(vec![concern("c1", vec![])]);
        inp.version = 2;
        let err = validate_concerns(&inp).unwrap_err();
        assert!(
            err.contains("version 1"),
            "error should name the supported version: {err}"
        );
    }

    #[test]
    fn validate_concerns_rejects_bad_id_patterns() {
        let long_id = "x".repeat(65);
        for bad in [
            "",
            "-leading-dash",
            ".leading-dot",
            "_leading-underscore",
            "has space",
            "emoji✨",
            long_id.as_str(),
        ] {
            let inp = input(vec![concern(bad, vec![])]);
            assert!(
                validate_concerns(&inp).is_err(),
                "id {bad:?} should be rejected"
            );
        }
        let ok = input(vec![concern("A1.b_c-d", vec![])]);
        assert!(validate_concerns(&ok).is_ok());
    }

    #[test]
    fn validate_concerns_rejects_bad_location_ranges() {
        let start_gt_end = input(vec![concern(
            "c1",
            vec![loc("a.ts", None, Some(5), Some(4))],
        )]);
        assert!(validate_concerns(&start_gt_end).is_err());

        let zero_start = input(vec![concern("c1", vec![loc("a.ts", None, Some(0), None)])]);
        assert!(validate_concerns(&zero_start).is_err());

        let zero_end = input(vec![concern("c1", vec![loc("a.ts", None, None, Some(0))])]);
        assert!(validate_concerns(&zero_end).is_err());

        let equal = input(vec![concern(
            "c1",
            vec![loc("a.ts", None, Some(3), Some(3))],
        )]);
        assert!(validate_concerns(&equal).is_ok());
    }

    #[test]
    fn validate_concerns_enforces_length_limits() {
        let mut long_title = concern("c1", vec![]);
        long_title.title = "x".repeat(201);
        assert!(validate_concerns(&input(vec![long_title])).is_err());

        let mut blank_title = concern("c1", vec![]);
        blank_title.title = "   ".to_string();
        assert!(validate_concerns(&input(vec![blank_title])).is_err());

        let mut long_desc = concern("c1", vec![]);
        long_desc.description = Some("x".repeat(20_001));
        assert!(validate_concerns(&input(vec![long_desc])).is_err());

        let mut long_summary = input(vec![concern("c1", vec![])]);
        long_summary.summary = Some("x".repeat(2_001));
        assert!(validate_concerns(&long_summary).is_err());

        let many = input(
            (0..201)
                .map(|i| concern(&format!("c{i}"), vec![]))
                .collect(),
        );
        assert!(validate_concerns(&many).is_err());

        let locs = (0..201).map(|_| loc("a.ts", None, None, None)).collect();
        assert!(validate_concerns(&input(vec![concern("c1", locs)])).is_err());
    }
}
