//! Immutable snapshot of what a review session is reviewing: the resolved
//! commit endpoints and canonical digests of the diff and concerns input.
//!
//! The snapshot is captured once at session start and never mutated. At
//! submit time the current `HEAD` is re-resolved and compared against
//! `head_oid` — if they differ the review is stale and submit is refused —
//! and the whole snapshot is embedded into the result JSON so a consumer can
//! tell exactly which commits and which concern set an approval applies to.

use crate::gitdiff::FileDiff;
use crate::model::ConcernsInput;
use sha2::{Digest, Sha256};

/// What a review session was pinned to when it started.
#[derive(Debug, Clone)]
pub struct ReviewSnapshot {
    /// The base ref exactly as the user passed it (`--base`).
    pub base_ref: String,
    /// Full oid the base ref resolved to at session start. `None` only when
    /// the session has no git repository behind it (the demo command).
    pub base_oid: Option<String>,
    /// Full oid `HEAD` resolved to at session start.
    pub head_oid: Option<String>,
    /// Full oid of `merge-base(base, HEAD)` — the diff's actual left side.
    pub merge_base_oid: Option<String>,
    /// SHA-256 (lowercase hex) of the canonical serialization of the diff
    /// shown to the reviewer. See [`diff_digest`].
    pub diff_sha256: String,
    /// SHA-256 (lowercase hex) of the canonical serialization of the full
    /// concerns input. See [`concerns_digest`].
    pub concerns_sha256: String,
}

impl ReviewSnapshot {
    /// Snapshot for a session with no git repository behind it (demo):
    /// digests are still computed, commit oids are absent.
    pub fn without_git(base_ref: &str, files: &[FileDiff], input: &ConcernsInput) -> Self {
        ReviewSnapshot {
            base_ref: base_ref.to_string(),
            base_oid: None,
            head_oid: None,
            merge_base_oid: None,
            diff_sha256: diff_digest(files),
            concerns_sha256: concerns_digest(input),
        }
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Canonical digest of the diff as shown to the reviewer.
///
/// The canonical form is the serde_json serialization of the `FileDiff`
/// slice: struct fields serialize in declaration order and the only
/// containers involved are `Vec`s (no maps), so equal diffs always produce
/// identical bytes. This covers paths, change/content kinds, modes, oids,
/// sizes, and every hunk line — any single-character change to what the
/// reviewer saw changes the digest.
pub fn diff_digest(files: &[FileDiff]) -> String {
    let json = serde_json::to_vec(files).expect("FileDiff serializes to JSON");
    sha256_hex(&json)
}

/// Canonical digest of the full concerns input (ids, titles, descriptions,
/// risks, locations), via its serde_json re-serialization — not the raw
/// input bytes, so formatting/whitespace differences in the source JSON do
/// not change the digest but any semantic change does.
pub fn concerns_digest(input: &ConcernsInput) -> String {
    let json = serde_json::to_vec(input).expect("ConcernsInput serializes to JSON");
    sha256_hex(&json)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gitdiff::parse_unified_diff;
    use crate::model::{Concern, Location, Risk};

    const DIFF: &str = "\
diff --git a/src/app.ts b/src/app.ts
index 1111111..2222222 100644
--- a/src/app.ts
+++ b/src/app.ts
@@ -1,3 +1,3 @@ header
 line1
-old2
+new2
 line3
";

    fn input(title: &str) -> ConcernsInput {
        ConcernsInput {
            version: 1,
            summary: None,
            concerns: vec![Concern {
                id: "c1".to_string(),
                title: title.to_string(),
                description: None,
                risk: Risk::Low,
                locations: vec![Location {
                    path: "src/app.ts".to_string(),
                    side: None,
                    start: None,
                    end: None,
                }],
            }],
        }
    }

    #[test]
    fn equal_inputs_produce_equal_digests() {
        let files_a = parse_unified_diff(DIFF);
        let files_b = parse_unified_diff(DIFF);
        assert_eq!(diff_digest(&files_a), diff_digest(&files_b));
        assert_eq!(concerns_digest(&input("t")), concerns_digest(&input("t")));
    }

    #[test]
    fn digests_are_lowercase_hex_sha256() {
        let d = diff_digest(&parse_unified_diff(DIFF));
        assert_eq!(d.len(), 64);
        assert!(d
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
    }

    #[test]
    fn one_character_diff_change_changes_digest() {
        let files_a = parse_unified_diff(DIFF);
        let files_b = parse_unified_diff(&DIFF.replace("new2", "new3"));
        assert_ne!(diff_digest(&files_a), diff_digest(&files_b));
    }

    #[test]
    fn one_character_concern_change_changes_digest() {
        assert_ne!(concerns_digest(&input("t")), concerns_digest(&input("u")));
    }

    #[test]
    fn concerns_digest_is_formatting_independent() {
        // Same semantic input parsed from differently formatted JSON must
        // digest identically (the digest is over the canonical
        // re-serialization, not the raw bytes).
        let compact = r#"{"version":1,"concerns":[{"id":"a","title":"t","risk":"low"}]}"#;
        let spaced = "{\n  \"version\": 1,\n  \"concerns\": [ { \"id\": \"a\", \"title\": \"t\", \"risk\": \"low\" } ]\n}";
        let a: ConcernsInput = serde_json::from_str(compact).unwrap();
        let b: ConcernsInput = serde_json::from_str(spaced).unwrap();
        assert_eq!(concerns_digest(&a), concerns_digest(&b));
    }
}
