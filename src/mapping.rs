//! Maps git diff hunks to agent-declared concerns.
//!
//! A hunk intersecting multiple concern locations belongs to all of them
//! (overlap is allowed). Hunks (or hunk-less files) claimed by no concern
//! are reported in `Mapping.unmapped`.

use crate::gitdiff::FileDiff;
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

/// The result of resolving every concern's locations against a diff.
#[derive(Debug)]
pub struct Mapping {
    pub concerns: Vec<MappedConcern>,
    pub unmapped: Vec<HunkRef>,
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

/// Resolves every concern's locations against the diff's files/hunks.
///
/// - `side: "new"` intersects a hunk's new range against `file.new_path`;
///   `side: "old"` intersects the old range against `file.old_path`.
/// - Missing `start`/`end` default to `1`/`u32::MAX` (i.e. the whole file).
/// - Hunk-less files (binary/pure rename) are only claimable by whole-file
///   locations (no `start` and no `end`), as `HunkRef { hunk: None }`.
/// - A location matching zero hunks produces a warning, never an error.
/// - Hunks (and hunk-less files) claimed by no concern land in `unmapped`.
pub fn resolve_mapping(files: &[FileDiff], input: &ConcernsInput) -> Mapping {
    let mut warnings = Vec::new();
    let mut claimed: std::collections::HashSet<HunkRef> = Default::default();
    let mut concerns = Vec::new();
    for c in &input.concerns {
        let mut refs: Vec<HunkRef> = Vec::new();
        for loc in &c.locations {
            let side = loc.side.unwrap_or(Side::New);
            let mut matched = false;
            for (fi, file) in files.iter().enumerate() {
                let path = match side {
                    Side::New => file.new_path.as_deref(),
                    Side::Old => file.old_path.as_deref(),
                };
                if path != Some(loc.path.as_str()) {
                    continue;
                }
                if file.hunks.is_empty() && loc.start.is_none() && loc.end.is_none() {
                    refs.push(HunkRef {
                        file: fi,
                        hunk: None,
                    });
                    matched = true;
                    continue;
                }
                for (hi, h) in file.hunks.iter().enumerate() {
                    let (hs, hc) = match side {
                        Side::New => (h.new_start, h.new_count),
                        Side::Old => (h.old_start, h.old_count),
                    };
                    let he = if hc == 0 {
                        hs
                    } else {
                        hs.saturating_add(hc - 1)
                    };
                    let ls = loc.start.unwrap_or(1);
                    let le = loc.end.unwrap_or(u32::MAX);
                    if hs.max(ls) <= he.min(le) {
                        refs.push(HunkRef {
                            file: fi,
                            hunk: Some(hi),
                        });
                        matched = true;
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
                warnings.push(format!("location matched no hunks: {}{}", loc.path, range));
            }
        }
        refs.sort_by_key(|r| (r.file, r.hunk));
        refs.dedup();
        claimed.extend(refs.iter().copied());
        concerns.push(MappedConcern {
            id: c.id.clone(),
            hunks: refs,
        });
    }
    let mut unmapped = Vec::new();
    for (fi, file) in files.iter().enumerate() {
        if file.hunks.is_empty() {
            let r = HunkRef {
                file: fi,
                hunk: None,
            };
            if !claimed.contains(&r) {
                unmapped.push(r);
            }
        } else {
            for hi in 0..file.hunks.len() {
                let r = HunkRef {
                    file: fi,
                    hunk: Some(hi),
                };
                if !claimed.contains(&r) {
                    unmapped.push(r);
                }
            }
        }
    }
    Mapping {
        concerns,
        unmapped,
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gitdiff::{ChangeKind, ContentKind, Hunk};
    use crate::model::{Concern, Location, Risk};

    /// Fabricates a `FileDiff` with `old_path == new_path == path` and one
    /// `Hunk` per `(old_start, old_count, new_start, new_count)` tuple, each
    /// with empty `lines`.
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
                .map(|&(old_start, old_count, new_start, new_count)| Hunk {
                    old_start,
                    old_count,
                    new_start,
                    new_count,
                    section: String::new(),
                    lines: Vec::new(),
                })
                .collect(),
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
        assert_eq!(mapping.warnings[0], "location matched no hunks: a.ts:20-");
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
                lines: Vec::new(),
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
            vec!["location matched no hunks: a.ts:5-10".to_string()]
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
            vec!["location matched no hunks: a.ts:-30".to_string()]
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
    fn zero_count_hunk_collapses_to_start_on_matched_side() {
        // Deletion hunk: new side has new_start=5, new_count=0, so its new
        // range collapses to [5, 5]. A side-new location 5-5 matches; 6-6
        // does not (the collapse changes the outcome: without it the range
        // would be empty or extend past 5).
        let files = vec![fd("a.ts", &[(5, 3, 5, 0)])];

        let hit = input(vec![concern(
            "c1",
            vec![loc("a.ts", None, Some(5), Some(5))],
        )]);
        let mapping_hit = resolve_mapping(&files, &hit);
        assert_eq!(
            mapping_hit.concerns[0].hunks,
            vec![HunkRef {
                file: 0,
                hunk: Some(0)
            }]
        );
        assert!(mapping_hit.warnings.is_empty());

        let miss = input(vec![concern(
            "c1",
            vec![loc("a.ts", None, Some(6), Some(6))],
        )]);
        let mapping_miss = resolve_mapping(&files, &miss);
        assert_eq!(mapping_miss.concerns[0].hunks, Vec::new());
        assert_eq!(
            mapping_miss.warnings,
            vec!["location matched no hunks: a.ts:6-6".to_string()]
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
