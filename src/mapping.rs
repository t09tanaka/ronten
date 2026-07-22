//! Maps git diff hunks to agent-declared concerns.
//!
//! A concern claims individual changed (`Add`/`Remove`) lines, not whole
//! hunks by range intersection — a location only claims a line whose own
//! line number falls in its range, never a hunk's context lines. The same
//! changed line may be claimed by multiple concerns (overlap is allowed).
//! A hunk is displayed under a concern once it claims at least one changed
//! line inside it. Changed lines (and hunk-less files) claimed by no
//! concern are reported in `Mapping.unmapped_lines` / `Mapping.unmapped`.

use crate::gitdiff::{FileDiff, GitError, LineKind, ResourceBudget};
use crate::model::{ConcernsInput, Severity, Side, Warning, SUPPORTED_VERSION};
use crate::termsafe::sanitize;
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
    pub warnings: Vec<Warning>,
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

/// A single semantic-validation failure from [`validate_concerns`]: a
/// stable, machine-readable `code`, a human-readable `message` (the same
/// text `review`'s startup path prints to stderr, via
/// [`format_validation_errors`]), and — for a failure scoped to one concern
/// — that concern's `id`. This is `Serialize`-derived so `ronten
/// validate-concerns` can emit it directly as JSON.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ValidationError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub concern_id: Option<String>,
}

impl ValidationError {
    fn new(code: &str, message: String) -> Self {
        ValidationError {
            code: code.to_string(),
            message,
            concern_id: None,
        }
    }

    fn for_concern(code: &str, concern_id: &str, message: String) -> Self {
        ValidationError {
            code: code.to_string(),
            message,
            concern_id: Some(concern_id.to_string()),
        }
    }
}

/// Joins every error's `message` (in order) with `"; "` into one
/// human-readable line — used by `review`'s startup path to print a single
/// stderr summary of the same failures `ronten validate-concerns` reports
/// structurally (one `ValidationError` per array entry).
pub fn format_validation_errors(errors: &[ValidationError]) -> String {
    errors
        .iter()
        .map(|e| e.message.as_str())
        .collect::<Vec<_>>()
        .join("; ")
}

/// Validates the concerns list itself (not against a diff): rejects an
/// unsupported contract version, an empty or oversized list, duplicate ids,
/// malformed ids (including the reserved `_unmapped`), over-long text
/// fields, and invalid location ranges.
///
/// Collects every failure found rather than stopping at the first: a caller
/// gets the complete set of problems in one pass (e.g. an unsupported
/// version *and* a duplicate id in the same input both show up), matching
/// what `ronten validate-concerns` reports in its `errors` array. Per
/// concern, mutually-exclusive checks on the same field (e.g. a reserved id
/// vs. a malformed id, or a location's `line == 0` vs. `start > end`) still
/// report only the first applicable problem for that field, to avoid
/// redundant errors describing the same root cause.
pub fn validate_concerns(input: &ConcernsInput) -> Result<(), Vec<ValidationError>> {
    let mut errors = Vec::new();

    if input.version != SUPPORTED_VERSION {
        errors.push(ValidationError::new(
            "UNSUPPORTED_VERSION",
            format!(
                "unsupported version {} (this ronten supports version {SUPPORTED_VERSION})",
                input.version
            ),
        ));
    }
    if input.concerns.is_empty() {
        errors.push(ValidationError::new(
            "EMPTY_CONCERNS",
            "concerns list must not be empty".to_string(),
        ));
    }
    if input.concerns.len() > 200 {
        errors.push(ValidationError::new(
            "TOO_MANY_CONCERNS",
            format!("too many concerns: {} (maximum 200)", input.concerns.len()),
        ));
    }
    if let Some(summary) = &input.summary {
        if summary.chars().count() > 2_000 {
            errors.push(ValidationError::new(
                "SUMMARY_TOO_LONG",
                "summary exceeds 2000 characters".to_string(),
            ));
        }
    }

    let mut seen = std::collections::HashSet::new();
    for c in &input.concerns {
        if c.id == UNMAPPED_ID {
            errors.push(ValidationError::for_concern(
                "RESERVED_CONCERN_ID",
                &c.id,
                format!("concern id \"{UNMAPPED_ID}\" is reserved and cannot be used"),
            ));
        } else if c.id.is_empty() || c.id.chars().count() > 64 || !valid_id_pattern(&c.id) {
            errors.push(ValidationError::for_concern(
                "INVALID_CONCERN_ID",
                &c.id,
                format!(
                    "invalid concern id {:?}: must be 1-64 characters matching ^[A-Za-z0-9][A-Za-z0-9._-]*$",
                    c.id
                ),
            ));
        } else if !seen.insert(c.id.as_str()) {
            errors.push(ValidationError::for_concern(
                "DUPLICATE_CONCERN_ID",
                &c.id,
                format!("duplicate concern id: {}", c.id),
            ));
        }

        if c.title.trim().is_empty() {
            errors.push(ValidationError::for_concern(
                "BLANK_TITLE",
                &c.id,
                format!("concern {:?}: title must not be blank", c.id),
            ));
        } else if c.title.chars().count() > 200 {
            errors.push(ValidationError::for_concern(
                "TITLE_TOO_LONG",
                &c.id,
                format!("concern {:?}: title exceeds 200 characters", c.id),
            ));
        }

        if let Some(description) = &c.description {
            if description.chars().count() > 20_000 {
                errors.push(ValidationError::for_concern(
                    "DESCRIPTION_TOO_LONG",
                    &c.id,
                    format!("concern {:?}: description exceeds 20000 characters", c.id),
                ));
            }
        }

        if c.locations.len() > 200 {
            errors.push(ValidationError::for_concern(
                "TOO_MANY_LOCATIONS",
                &c.id,
                format!(
                    "concern {:?}: too many locations: {} (maximum 200)",
                    c.id,
                    c.locations.len()
                ),
            ));
        }

        for loc in &c.locations {
            if loc.start == Some(0) || loc.end == Some(0) {
                errors.push(ValidationError::for_concern(
                    "INVALID_LINE_NUMBER",
                    &c.id,
                    format!(
                        "concern {:?}: location {}: line numbers are 1-based (0 is invalid)",
                        c.id,
                        sanitize(&loc.path)
                    ),
                ));
            } else if let (Some(start), Some(end)) = (loc.start, loc.end) {
                if start > end {
                    errors.push(ValidationError::for_concern(
                        "START_AFTER_END",
                        &c.id,
                        format!(
                            "concern {:?}: location {}: start {start} is greater than end {end}",
                            c.id,
                            sanitize(&loc.path)
                        ),
                    ));
                }
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Sort key for `UnmappedLine.side` in `(file, side, line)` order (`Old`
/// before `New`); `Side` itself has no `Ord` since nothing else needs one.
fn side_sort_key(side: Side) -> u8 {
    match side {
        Side::Old => 0,
        Side::New => 1,
    }
}

/// Merges a list of inclusive `[start, end]` `u32` intervals into the
/// minimal disjoint, start-sorted set covering the same union of line
/// numbers. Overlapping, fully-contained, adjacent (touching with no gap),
/// and out-of-order input all collapse correctly.
///
/// Used to collapse a concern's (possibly many, possibly duplicate or
/// overlapping) locations on the same `(path, side)` into the minimal set
/// of ranges actually walked against a file's changed lines, so a
/// pathological concern with hundreds of overlapping/duplicate locations
/// re-walks each changed line at most once instead of once per location.
/// This can never change which lines get claimed: the walk only ever
/// registers a changed line that already exists in one of the merged
/// ranges' union, which by construction equals the union of the original
/// (unmerged) ranges.
fn merge_intervals(mut intervals: Vec<(u32, u32)>) -> Vec<(u32, u32)> {
    intervals.sort_unstable_by_key(|&(start, _)| start);
    let mut merged: Vec<(u32, u32)> = Vec::with_capacity(intervals.len());
    for (start, end) in intervals {
        match merged.last_mut() {
            // `start <= last_end + 1` (saturating, since `end` may be
            // `u32::MAX`) treats touching intervals — e.g. `[1,5]` and
            // `[6,10]` — as mergeable: walking the merged `[1,10]` visits
            // exactly the same changed lines as walking both separately,
            // since changed lines (not the interval itself) are the walk
            // target, so merging adjacent ranges can never claim a line
            // neither original range could have claimed.
            Some(&mut (_, ref mut last_end)) if start <= last_end.saturating_add(1) => {
                if end > *last_end {
                    *last_end = end;
                }
            }
            _ => merged.push((start, end)),
        }
    }
    merged
}

/// A concern's locations on one `(path, side)`, accumulated before merging:
/// every location's resolved `[start, end]` range, plus whether any of them
/// was a true whole-file location (no `start` and no `end`) — tracked
/// separately since that gates hunk-less whole-file capture independently
/// of the numeric range a `None`/`None` location resolves to.
#[derive(Default)]
struct LocationGroup {
    intervals: Vec<(u32, u32)>,
    whole_file: bool,
}

/// Whether `sorted` (ascending by line number, as `adds`/`removes` are kept)
/// contains at least one entry in `[start, end]`, via a single binary
/// search — O(log n) regardless of how many entries the range would
/// contain. Used for the per-location "matched nothing" warning check,
/// which must reflect each location's own range individually and so can't
/// be answered from the (merged, per-group) claim walk.
fn range_has_entry(sorted: &[(usize, u32)], start: u32, end: u32) -> bool {
    let lo = sorted.partition_point(|&(_, no)| no < start);
    sorted.get(lo).is_some_and(|&(_, no)| no <= end)
}

// Test-only instrumentation: the total changed-line claim registrations
// (`resolved_edges`) performed by the most recent `resolve_mapping` /
// `resolve_mapping_with_budget` call on the current thread. Lets tests
// assert the interval-merge optimization actually bounds the walk to
// O(changed lines) per concern rather than O(locations x changed lines),
// without exposing the counter on the public `Mapping` type. Safe across
// tests: the test harness runs one test function to completion (no
// interleaving with another test's `resolve_mapping` call) before reusing a
// worker thread, and this is unconditionally overwritten on every call.
#[cfg(test)]
thread_local! {
    static LAST_RESOLVED_EDGES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn last_resolved_edges() -> usize {
    LAST_RESOLVED_EDGES.with(|c| c.get())
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
///
/// Resolves against the default [`ResourceBudget`]; see
/// [`resolve_mapping_with_budget`] for a caller-supplied budget.
pub fn resolve_mapping(files: &[FileDiff], input: &ConcernsInput) -> Result<Mapping, GitError> {
    resolve_mapping_with_budget(files, input, &ResourceBudget::default())
}

/// [`resolve_mapping`] with an explicit [`ResourceBudget`] (the default
/// budget minus the ability to override it in tests, which need adversarial
/// fixtures — or a tiny budget — to exercise the hard caps below without
/// timing out).
///
/// Two hard caps, on top of everything [`resolve_mapping`] already
/// documents, bound the concern x location sweep itself: up to 200 concerns
/// x 200 locations each is legal input, and a pathological-but-legal
/// instance (e.g. every location pointing at the same maximal file) could
/// otherwise multiply into an unbounded amount of work even after
/// per-concern interval merging collapses duplicate/overlapping locations
/// within a single concern. `budget.max_resolved_edges` bounds the total
/// claimed changed-line registrations across *all* concerns (merging can't
/// help across concerns — 200 concerns each fully claiming the same large
/// file is still 200x the work); `budget.max_hunk_refs` bounds the total
/// `HunkRef` entries across all concerns' displayed hunks. Either cap being
/// exceeded returns `GitError::BudgetExceeded` — the review is refused
/// before an oversized session is built, never silently truncated.
pub fn resolve_mapping_with_budget(
    files: &[FileDiff],
    input: &ConcernsInput,
    budget: &ResourceBudget,
) -> Result<Mapping, GitError> {
    use std::collections::{BTreeSet, HashMap, HashSet};

    // Step 1: per-file changed-line lists, and path -> file-index maps so
    // the concern/location sweep below never linearly rescans `files`.
    let mut adds: Vec<Vec<(usize, u32)>> = vec![Vec::new(); files.len()];
    let mut removes: Vec<Vec<(usize, u32)>> = vec![Vec::new(); files.len()];
    let mut by_new_path: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut by_old_path: HashMap<&str, Vec<usize>> = HashMap::new();
    // (file, hunk) pairs containing a changed line with no line number
    // (`Add { new_no: None }` / `Remove { old_no: None }`). The parser is
    // expected to always supply a number for changed lines (checked by the
    // debug_assert below in development builds), but that invariant must
    // never be load-bearing in release builds: a line we can't place on
    // either side can never be claimed (it isn't in `adds`/`removes`, so no
    // location's range can ever match it), so its hunk is force-unioned into
    // `unmapped` below rather than silently vanishing from both the claim
    // set and the unmapped view.
    let mut forced_unmapped: HashSet<(usize, usize)> = HashSet::new();
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
                        // The parser guarantees every Add line carries
                        // new_no; `unmapped`/`unmapped_lines` completeness
                        // below depends on that invariant holding. Skipped
                        // under `cfg(test)` so the regression test below can
                        // exercise the `None` fallback directly, without the
                        // fabricated invariant break aborting the test first
                        // — normal (non-test) debug/dev builds still panic.
                        #[cfg(not(test))]
                        debug_assert!(line.new_no.is_some(), "Add line missing new_no");
                        match line.new_no {
                            Some(no) => adds[fi].push((hi, no)),
                            None => {
                                forced_unmapped.insert((fi, hi));
                            }
                        }
                    }
                    LineKind::Remove => {
                        // Same invariant as above, for old_no on Remove lines.
                        #[cfg(not(test))]
                        debug_assert!(line.old_no.is_some(), "Remove line missing old_no");
                        match line.old_no {
                            Some(no) => removes[fi].push((hi, no)),
                            None => {
                                forced_unmapped.insert((fi, hi));
                            }
                        }
                    }
                    LineKind::Context => {}
                }
            }
        }
    }

    // The scans in step 3 range-query these by line number; sort so a
    // location claims its range via binary search + bounded walk instead of
    // a full scan (up to 200 concerns x 200 locations each would otherwise
    // rescan every changed line of the file per location). Hunks emit lines
    // in ascending order already, so this is usually a no-op.
    for per_file in adds.iter_mut().chain(removes.iter_mut()) {
        per_file.sort_unstable_by_key(|&(_, no)| no);
    }

    // Step 2: claim sets.
    let mut warnings = Vec::new();
    let mut claimed_adds: HashSet<(usize, u32)> = HashSet::new();
    let mut claimed_removes: HashSet<(usize, u32)> = HashSet::new();
    let mut claimed_hunkless: HashSet<usize> = HashSet::new();
    let mut concerns = Vec::new();

    // Step 3: each concern's each location. Split into two passes:
    //
    // 3a. Per-location "matched nothing" detection, via a single O(log n)
    //     existence check per location — cheap and independent of how large
    //     the location's range is, so it stays correct (and bounded) without
    //     needing the merge below.
    // 3b. Per-(path, side) interval merge, then a claim walk over the
    //     *merged* intervals only. A pathological concern with up to 200
    //     overlapping/duplicate locations on the same file collapses to a
    //     handful of merged ranges instead of re-walking the file's changed
    //     lines once per location; see `merge_intervals` for why this can't
    //     change which lines get claimed. `resolved_edges` and
    //     `total_hunk_refs` accumulate across *all* concerns (merging is a
    //     per-concern optimization — it can't stop 200 different concerns
    //     from each fully claiming the same large file), enforcing the hard
    //     caps that bound that cross-concern multiplication.
    let mut resolved_edges: usize = 0;
    let mut total_hunk_refs: usize = 0;
    for c in &input.concerns {
        let mut covered_hunks: BTreeSet<(usize, Option<usize>)> = BTreeSet::new();

        // 3a: warnings, computed per original location.
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
                    matched = true;
                    continue;
                }
                if want_adds && range_has_entry(&adds[fi], start, end) {
                    matched = true;
                }
                if want_removes && range_has_entry(&removes[fi], start, end) {
                    matched = true;
                }
            }

            if !matched {
                let range = match (loc.start, loc.end) {
                    (Some(s), Some(e)) => format!(":{s}-{e}"),
                    (Some(s), None) => format!(":{s}-"),
                    (None, Some(e)) => format!(":-{e}"),
                    (None, None) => String::new(),
                };
                warnings.push(
                    Warning::new(
                        "LOCATION_MATCHED_NOTHING",
                        Severity::Warning,
                        format!("location matched no changed lines: {}{}", loc.path, range),
                    )
                    .with_path(loc.path.clone())
                    .with_concern(c.id.clone()),
                );
            }
        }

        // 3b: group locations by (path, side), merge each group's
        // intervals, then walk the merged intervals once to register
        // claims.
        let mut groups: HashMap<(&str, Option<Side>), LocationGroup> = HashMap::new();
        for loc in &c.locations {
            let start = loc.start.unwrap_or(1);
            let end = loc.end.unwrap_or(u32::MAX);
            let whole_file = loc.start.is_none() && loc.end.is_none();
            let group = groups.entry((loc.path.as_str(), loc.side)).or_default();
            group.intervals.push((start, end));
            group.whole_file |= whole_file;
        }

        for ((path, side), group) in groups {
            let LocationGroup {
                intervals,
                whole_file,
            } = group;
            let file_indices: &[usize] = match side.unwrap_or(Side::New) {
                Side::New => by_new_path.get(path),
                Side::Old => by_old_path.get(path),
            }
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
            let want_adds = !matches!(side, Some(Side::Old));
            let want_removes = !matches!(side, Some(Side::New));
            let merged = merge_intervals(intervals);

            for &fi in file_indices {
                let file = &files[fi];
                if file.hunks.is_empty() {
                    if whole_file {
                        covered_hunks.insert((fi, None));
                        claimed_hunkless.insert(fi);
                    }
                    continue;
                }
                for &(start, end) in &merged {
                    if want_adds {
                        let lo = adds[fi].partition_point(|&(_, no)| no < start);
                        for &(hi, no) in &adds[fi][lo..] {
                            if no > end {
                                break;
                            }
                            resolved_edges += 1;
                            if resolved_edges > budget.max_resolved_edges {
                                return Err(GitError::BudgetExceeded(format!(
                                    "too many resolved edges: over {} claimed changed-line \
                                     registrations across all concerns (maximum {})",
                                    resolved_edges, budget.max_resolved_edges
                                )));
                            }
                            claimed_adds.insert((fi, no));
                            covered_hunks.insert((fi, Some(hi)));
                        }
                    }
                    if want_removes {
                        let lo = removes[fi].partition_point(|&(_, no)| no < start);
                        for &(hi, no) in &removes[fi][lo..] {
                            if no > end {
                                break;
                            }
                            resolved_edges += 1;
                            if resolved_edges > budget.max_resolved_edges {
                                return Err(GitError::BudgetExceeded(format!(
                                    "too many resolved edges: over {} claimed changed-line \
                                     registrations across all concerns (maximum {})",
                                    resolved_edges, budget.max_resolved_edges
                                )));
                            }
                            claimed_removes.insert((fi, no));
                            covered_hunks.insert((fi, Some(hi)));
                        }
                    }
                }
            }
        }

        // Step 4: concern's display hunks, sorted (file, hunk).
        let hunks: Vec<HunkRef> = covered_hunks
            .into_iter()
            .map(|(file, hunk)| HunkRef { file, hunk })
            .collect();
        total_hunk_refs += hunks.len();
        if total_hunk_refs > budget.max_hunk_refs {
            return Err(GitError::BudgetExceeded(format!(
                "too many hunk refs: over {} displayed hunk references across all concerns \
                 (maximum {})",
                total_hunk_refs, budget.max_hunk_refs
            )));
        }
        concerns.push(MappedConcern {
            id: c.id.clone(),
            hunks,
        });
    }

    #[cfg(test)]
    LAST_RESOLVED_EDGES.with(|c| c.set(resolved_edges));

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
                if unmapped_hunks_set.contains(&(fi, hi)) || forced_unmapped.contains(&(fi, hi)) {
                    unmapped.push(HunkRef {
                        file: fi,
                        hunk: Some(hi),
                    });
                }
            }
        }
    }

    Ok(Mapping {
        concerns,
        unmapped,
        unmapped_lines,
        warnings,
    })
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
            old_type: None,
            new_type: None,
            old_oid: None,
            new_oid: None,
            old_size: None,
            new_size: None,
            lfs_pointer: false,
            hunks: hunks
                .iter()
                .map(|&(old_start, old_count, new_start, new_count)| {
                    let mut lines = Vec::new();
                    for i in 0..old_count {
                        lines.push(DiffLine {
                            kind: LineKind::Remove,
                            content: String::new(),
                            eol: crate::gitdiff::Eol::Lf,
                            old_no: Some(old_start + i),
                            new_no: None,
                        });
                    }
                    for i in 0..new_count {
                        lines.push(DiffLine {
                            kind: LineKind::Add,
                            content: String::new(),
                            eol: crate::gitdiff::Eol::Lf,
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
                eol: crate::gitdiff::Eol::Lf,
                old_no,
                new_no,
            })
            .collect();
        for &n in context_lines {
            lines.push(DiffLine {
                kind: LineKind::Context,
                content: String::new(),
                eol: crate::gitdiff::Eol::Lf,
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
            old_type: None,
            new_type: None,
            old_oid: None,
            new_oid: None,
            old_size: None,
            new_size: None,
            lfs_pointer: false,
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
        let mapping = resolve_mapping(&files, &inp).unwrap();
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
        let mapping = resolve_mapping(&files, &inp).unwrap();
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
            mapping.warnings[0].message,
            "location matched no changed lines: a.ts:20-"
        );
        assert_eq!(mapping.warnings[0].code, "LOCATION_MATCHED_NOTHING");
        assert_eq!(mapping.warnings[0].concern_id.as_deref(), Some("c1"));
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
            old_type: None,
            new_type: None,
            old_oid: None,
            new_oid: None,
            old_size: None,
            new_size: None,
            lfs_pointer: false,
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
                        eol: crate::gitdiff::Eol::Lf,
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
        let mapping = resolve_mapping(&files, &inp).unwrap();
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
        let mapping = resolve_mapping(&files, &inp).unwrap();
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
        let mapping = resolve_mapping(&files, &inp).unwrap();
        assert_eq!(
            mapping.unmapped,
            vec![HunkRef {
                file: 0,
                hunk: Some(1)
            }]
        );

        // Whole-file location claims both hunks -> nothing unmapped.
        let inp_full = input(vec![concern("c1", vec![loc("a.ts", None, None, None)])]);
        let mapping_full = resolve_mapping(&files, &inp_full).unwrap();
        assert!(mapping_full.unmapped.is_empty());
    }

    #[test]
    fn location_matching_zero_hunks_warns_without_error() {
        let files = vec![fd("a.ts", &[(1, 2, 1, 2)])];
        let inp = input(vec![concern(
            "c1",
            vec![loc("a.ts", None, Some(5), Some(10))],
        )]);
        let mapping = resolve_mapping(&files, &inp).unwrap();
        assert_eq!(mapping.concerns[0].hunks, Vec::new());
        assert_eq!(
            mapping
                .warnings
                .iter()
                .map(|w| w.message.clone())
                .collect::<Vec<_>>(),
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
                old_type: None,
                new_type: None,
                old_oid: None,
                new_oid: None,
                old_size: None,
                new_size: None,
                lfs_pointer: false,
                hunks: Vec::new(),
            },
            FileDiff {
                old_path: Some("other.png".to_string()),
                new_path: Some("other.png".to_string()),
                change_kind: ChangeKind::Modified,
                content_kind: ContentKind::Binary,
                old_mode: None,
                new_mode: None,
                old_type: None,
                new_type: None,
                old_oid: None,
                new_oid: None,
                old_size: None,
                new_size: None,
                lfs_pointer: false,
                hunks: Vec::new(),
            },
        ];
        let inp = input(vec![concern("c1", vec![loc("logo.png", None, None, None)])]);
        let mapping = resolve_mapping(&files, &inp).unwrap();
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
        let mapping = resolve_mapping(&files, &inp).unwrap();
        assert_eq!(mapping.concerns[0].hunks, Vec::new());
        assert_eq!(
            mapping
                .warnings
                .iter()
                .map(|w| w.message.clone())
                .collect::<Vec<_>>(),
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
        let mapping = resolve_mapping(&files, &inp).unwrap();
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
        let mapping_new = resolve_mapping(&files, &new_side).unwrap();
        assert_eq!(mapping_new.concerns[0].hunks, Vec::new());
        assert_eq!(
            mapping_new
                .warnings
                .iter()
                .map(|w| w.message.clone())
                .collect::<Vec<_>>(),
            vec!["location matched no changed lines: a.ts:5-5".to_string()]
        );

        // side=Old over the full removed range claims all three lines.
        let old_side = input(vec![concern(
            "c1",
            vec![loc("a.ts", Some(Side::Old), Some(5), Some(7))],
        )]);
        let mapping_old = resolve_mapping(&files, &old_side).unwrap();
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
        let mapping = resolve_mapping(&files, &inp).unwrap();
        assert!(mapping.concerns[0].hunks.is_empty());
        assert_eq!(mapping.warnings.len(), 1);
        assert!(mapping.warnings[0]
            .message
            .contains("matched no changed lines"));
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
        let mapping = resolve_mapping(&files, &inp).unwrap();
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
    fn split_hunk_across_two_concerns() {
        // One hunk with two Add lines (new 13, new 15). Concern A claims 13,
        // concern B claims 15 — the hunk must show up under BOTH concerns'
        // hunks, and every line is claimed so nothing is unmapped.
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
        let inp = input(vec![
            concern("a", vec![loc("a.ts", None, Some(13), Some(13))]),
            concern("b", vec![loc("a.ts", None, Some(15), Some(15))]),
        ]);
        let mapping = resolve_mapping(&files, &inp).unwrap();
        assert_eq!(
            mapping.concerns[0].hunks,
            vec![HunkRef {
                file: 0,
                hunk: Some(0)
            }]
        );
        assert_eq!(
            mapping.concerns[1].hunks,
            vec![HunkRef {
                file: 0,
                hunk: Some(0)
            }]
        );
        assert!(mapping.unmapped.is_empty());
        assert!(mapping.unmapped_lines.is_empty());
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
        let mapping = resolve_mapping(&files, &inp).unwrap();
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
        let mapping = resolve_mapping(&files, &inp).unwrap();
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
        let errors = validate_concerns(&inp).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| e.code == "UNSUPPORTED_VERSION" && e.message.contains("version 1")),
            "errors should include an UNSUPPORTED_VERSION error naming the supported version: {errors:?}"
        );
    }

    #[test]
    fn validate_concerns_reports_stable_error_codes() {
        let dup = input(vec![concern("c1", vec![]), concern("c1", vec![])]);
        let errors = validate_concerns(&dup).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| e.code == "DUPLICATE_CONCERN_ID" && e.concern_id.as_deref() == Some("c1")),
            "expected a DUPLICATE_CONCERN_ID error for c1: {errors:?}"
        );

        let start_after_end = input(vec![concern(
            "c1",
            vec![loc("a.ts", None, Some(5), Some(4))],
        )]);
        let errors = validate_concerns(&start_after_end).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|e| e.code == "START_AFTER_END" && e.concern_id.as_deref() == Some("c1")),
            "expected a START_AFTER_END error for c1: {errors:?}"
        );
    }

    #[test]
    fn validate_concerns_collects_every_error_not_just_the_first() {
        // Two independent, unrelated problems (an unsupported top-level
        // version and a duplicate concern id) must both surface, not just
        // whichever one the validator happens to check first.
        let mut inp = input(vec![concern("c1", vec![]), concern("c1", vec![])]);
        inp.version = 2;
        let errors = validate_concerns(&inp).unwrap_err();
        assert!(errors.iter().any(|e| e.code == "UNSUPPORTED_VERSION"));
        assert!(errors.iter().any(|e| e.code == "DUPLICATE_CONCERN_ID"));
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

    #[test]
    fn numberless_changed_line_forces_its_hunk_into_unmapped() {
        // A hunk with an ordinary Add (new 13) plus a pathological changed
        // line carrying no line number at all (`Add { new_no: None }` —
        // should never happen past the parser, but must fail closed, not
        // open, if it ever does). A whole-file location claims every
        // *numbered* changed line in the file; the malformed line can never
        // be claimed (it's in no claim set any location could match), so its
        // hunk must still show up in `unmapped` even though the location
        // claimed everything it could see.
        let files = vec![fd_with_lines(
            "a.ts",
            13,
            1,
            13,
            1,
            &[(LineKind::Add, None, Some(13)), (LineKind::Add, None, None)],
            &[],
        )];
        let inp = input(vec![concern("c1", vec![loc("a.ts", None, None, None)])]);
        let mapping = resolve_mapping(&files, &inp).unwrap();
        assert_eq!(
            mapping.concerns[0].hunks,
            vec![HunkRef {
                file: 0,
                hunk: Some(0)
            }],
            "the location did claim the one numbered line in the hunk"
        );
        assert_eq!(
            mapping.unmapped,
            vec![HunkRef {
                file: 0,
                hunk: Some(0)
            }],
            "a hunk containing an unclaimable numberless line must stay in \
             unmapped even though every claimable line in it was claimed"
        );
    }

    #[test]
    fn deleted_file_whole_file_old_side_claims_removes_unspecified_side_warns() {
        // Fix 6 regression: a deleted file (old_path only, remove lines
        // only). `side: "old"` whole-file location must claim every remove;
        // an unspecified-side location resolves file identity via new_path,
        // which doesn't exist for a deletion, so it must warn instead.
        let files = vec![FileDiff {
            old_path: Some("gone.txt".to_string()),
            new_path: None,
            change_kind: ChangeKind::Deleted,
            content_kind: ContentKind::Text,
            old_mode: None,
            new_mode: None,
            old_type: None,
            new_type: None,
            old_oid: None,
            new_oid: None,
            old_size: None,
            new_size: None,
            lfs_pointer: false,
            hunks: vec![Hunk {
                old_start: 1,
                old_count: 3,
                new_start: 0,
                new_count: 0,
                section: String::new(),
                lines: (1..=3)
                    .map(|n| DiffLine {
                        kind: LineKind::Remove,
                        content: String::new(),
                        eol: crate::gitdiff::Eol::Lf,
                        old_no: Some(n),
                        new_no: None,
                    })
                    .collect(),
            }],
        }];

        let old_side = input(vec![concern(
            "c1",
            vec![loc("gone.txt", Some(Side::Old), None, None)],
        )]);
        let mapping_old = resolve_mapping(&files, &old_side).unwrap();
        assert_eq!(
            mapping_old.concerns[0].hunks,
            vec![HunkRef {
                file: 0,
                hunk: Some(0)
            }]
        );
        assert!(mapping_old.warnings.is_empty());
        assert!(mapping_old.unmapped.is_empty());

        let unspecified_side = input(vec![concern("c1", vec![loc("gone.txt", None, None, None)])]);
        let mapping_unspecified = resolve_mapping(&files, &unspecified_side).unwrap();
        assert!(mapping_unspecified.concerns[0].hunks.is_empty());
        assert_eq!(mapping_unspecified.warnings.len(), 1);
        assert_eq!(
            mapping_unspecified.unmapped,
            vec![HunkRef {
                file: 0,
                hunk: Some(0)
            }]
        );
    }

    #[test]
    fn rename_new_path_location_with_unspecified_side_also_claims_old_side_removes() {
        // Fix 6 regression: a rename with content changes (old_path !=
        // new_path). A whole-file location naming the *new* path with no
        // side resolves file identity via new_path, but still claims
        // Remove lines (keyed by old_no) belonging to that same file index,
        // since an unspecified side wants both adds and removes.
        let files = vec![FileDiff {
            old_path: Some("old.ts".to_string()),
            new_path: Some("new.ts".to_string()),
            change_kind: ChangeKind::Renamed,
            content_kind: ContentKind::Text,
            old_mode: None,
            new_mode: None,
            old_type: None,
            new_type: None,
            old_oid: None,
            new_oid: None,
            old_size: None,
            new_size: None,
            lfs_pointer: false,
            hunks: vec![Hunk {
                old_start: 10,
                old_count: 1,
                new_start: 10,
                new_count: 1,
                section: String::new(),
                lines: vec![
                    DiffLine {
                        kind: LineKind::Remove,
                        content: String::new(),
                        eol: crate::gitdiff::Eol::Lf,
                        old_no: Some(10),
                        new_no: None,
                    },
                    DiffLine {
                        kind: LineKind::Add,
                        content: String::new(),
                        eol: crate::gitdiff::Eol::Lf,
                        old_no: None,
                        new_no: Some(10),
                    },
                ],
            }],
        }];
        let inp = input(vec![concern("c1", vec![loc("new.ts", None, None, None)])]);
        let mapping = resolve_mapping(&files, &inp).unwrap();
        assert_eq!(
            mapping.concerns[0].hunks,
            vec![HunkRef {
                file: 0,
                hunk: Some(0)
            }]
        );
        assert!(mapping.warnings.is_empty());
        assert!(
            mapping.unmapped.is_empty(),
            "the remove line (old_no-keyed) must have been claimed too, not just the add"
        );
        assert!(mapping.unmapped_lines.is_empty());
    }

    #[test]
    fn interval_merge_handles_overlap_containment_adjacency_and_reversed_order() {
        // Fed out of order on purpose. Expected collapse:
        //   (1,5)+(3,8)        overlap    -> (1,8)
        //   (10,20)+(12,15)    containment -> (10,20)
        //   ...+(21,25)+(26,30) adjacency  -> (10,30) (touching, no gap)
        //   (50,60)            disjoint (real gap) -> stays separate
        let merged = merge_intervals(vec![
            (26, 30),
            (3, 8),
            (12, 15),
            (50, 60),
            (1, 5),
            (21, 25),
            (10, 20),
        ]);
        assert_eq!(merged, vec![(1, 8), (10, 30), (50, 60)]);
    }

    #[test]
    fn adversarial_40k_locations_same_file_is_bounded() {
        // 200 concerns x 200 locations (the legal maximum of each, per
        // `validate_concerns`), every single one a whole-file location on
        // the same file with N changed (Add) lines. Without per-concern
        // interval merging this would walk the file's changed lines once
        // per location: 200 x 200 x N. With merging, each concern's 200
        // identical whole-file locations collapse to a single merged range,
        // so the walk is only 200 x N.
        const N: u32 = 3_000;
        let files = vec![fd("big.ts", &[(1, 0, 1, N)])];
        let locations: Vec<Location> = (0..200).map(|_| loc("big.ts", None, None, None)).collect();
        let concerns: Vec<Concern> = (0..200)
            .map(|i| concern(&format!("c{i}"), locations.clone()))
            .collect();
        let inp = input(concerns);

        let mapping = resolve_mapping(&files, &inp).expect("bounded by the default budget");

        // Correctness: every concern claims the file's one hunk, and every
        // changed line is claimed -- the same result a single [1,N]
        // location per concern would have produced.
        assert_eq!(mapping.concerns.len(), 200);
        for c in &mapping.concerns {
            assert_eq!(
                c.hunks,
                vec![HunkRef {
                    file: 0,
                    hunk: Some(0)
                }]
            );
        }
        assert!(mapping.warnings.is_empty());
        assert!(mapping.unmapped.is_empty());
        assert!(mapping.unmapped_lines.is_empty());

        // Boundedness: the walk counter proves the merge actually ran --
        // 200 concerns x N, not 200 x 200 x N (which would be 120,000,000,
        // far past the default 1,000,000-edge budget).
        assert_eq!(last_resolved_edges(), 200 * N as usize);
    }

    #[test]
    fn resolved_edges_cap_refuses() {
        let files = vec![fd("a.ts", &[(1, 0, 1, 10)])]; // 10 Add lines
        let inp = input(vec![concern("c1", vec![loc("a.ts", None, None, None)])]);
        let budget = crate::gitdiff::ResourceBudget {
            max_resolved_edges: 5,
            ..crate::gitdiff::ResourceBudget::default()
        };
        match resolve_mapping_with_budget(&files, &inp, &budget) {
            Err(GitError::BudgetExceeded(msg)) => {
                assert!(msg.contains("resolved edges"), "unexpected message: {msg}");
            }
            other => panic!("expected BudgetExceeded, got {other:?}"),
        }
    }

    #[test]
    fn hunk_refs_cap_refuses() {
        // 3 separate hunks, one concern claiming all of them via a
        // whole-file location -> 3 hunk refs, over a budget of 2.
        let files = vec![fd("a.ts", &[(1, 1, 1, 1), (10, 1, 10, 1), (20, 1, 20, 1)])];
        let inp = input(vec![concern("c1", vec![loc("a.ts", None, None, None)])]);
        let budget = crate::gitdiff::ResourceBudget {
            max_hunk_refs: 2,
            ..crate::gitdiff::ResourceBudget::default()
        };
        match resolve_mapping_with_budget(&files, &inp, &budget) {
            Err(GitError::BudgetExceeded(msg)) => {
                assert!(msg.contains("hunk refs"), "unexpected message: {msg}");
            }
            other => panic!("expected BudgetExceeded, got {other:?}"),
        }
    }
}
