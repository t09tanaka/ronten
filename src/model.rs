//! Data model for the concerns input contract and result output contract.
//!
//! Unknown fields on input types are rejected (`deny_unknown_fields`), since
//! the contract's version is pinned to 1: a typo like `statr` must not
//! silently be dropped and widen a location to whole-file. Any intentional
//! extension happens in a future version 2, not by loosening this one.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The only concerns *input* contract version this build of ronten accepts.
pub const SUPPORTED_VERSION: u32 = 1;

/// The result *output* contract version this build of ronten emits. v2 added
/// the `review` block pinning the result to the reviewed commits and to
/// canonical digests of the diff and concerns input.
pub const OUTPUT_VERSION: u32 = 2;

/// Top-level input: a list of concerns an agent wants a human to review.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConcernsInput {
    /// Contract version. Must be exactly 1 (the only supported version).
    #[schemars(range(min = 1, max = 1))]
    pub version: u32,
    /// Optional overall summary of the change (at most 2000 characters).
    #[serde(default)]
    #[schemars(length(max = 2000))]
    pub summary: Option<String>,
    /// The concerns to review: 1 to 200 entries with unique ids.
    #[schemars(length(min = 1, max = 200))]
    pub concerns: Vec<Concern>,
}

/// A single concern raised against the diff.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Concern {
    /// Unique concern id: 1-64 characters matching
    /// `^[A-Za-z0-9][A-Za-z0-9._-]*$`. The id `_unmapped` is reserved.
    #[schemars(length(min = 1, max = 64), pattern(r"^[A-Za-z0-9][A-Za-z0-9._-]*$"))]
    pub id: String,
    /// Short title (at most 200 characters; must be non-blank after
    /// trimming whitespace).
    #[schemars(length(min = 1, max = 200))]
    pub title: String,
    /// Longer description (at most 20000 characters).
    #[serde(default)]
    #[schemars(length(max = 20000))]
    pub description: Option<String>,
    pub risk: Risk,
    /// Diff locations this concern covers (at most 200 entries).
    #[serde(default)]
    #[schemars(length(max = 200))]
    pub locations: Vec<Location>,
}

/// Risk level assigned to a concern.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Risk {
    High,
    Medium,
    Low,
}

/// A location within the diff that a concern points at.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Location {
    pub path: String,
    #[serde(default)]
    pub side: Option<Side>,
    /// First line of the range, 1-based (0 is invalid). When both `start`
    /// and `end` are present, `start` must be <= `end`.
    #[serde(default)]
    #[schemars(range(min = 1))]
    pub start: Option<u32>,
    /// Last line of the range, 1-based and inclusive (0 is invalid; must be
    /// >= `start` when both are present).
    #[serde(default)]
    #[schemars(range(min = 1))]
    pub end: Option<u32>,
}

/// Which side of a diff a location or comment refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Old,
    New,
}

/// Top-level output: the human's decision plus per-concern verdicts, pinned
/// to exactly what was reviewed via the `review` block.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ResultOutput {
    pub version: u32,
    /// What this result applies to: the reviewed commits and digests of the
    /// diff and concerns input, plus the assurance level of the outcome.
    pub review: ReviewInfo,
    pub decision: Decision,
    pub concerns: Vec<ConcernResult>,
    pub general_comments: Vec<String>,
    pub warnings: Vec<Warning>,
    pub started_at: String,
    pub submitted_at: String,
}

/// A structured warning surfaced to the reviewer and preserved in the
/// result, so a later audit can tell programmatically what the review
/// session flagged (not just as prose).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Warning {
    /// Stable machine-readable code, e.g. `FILE_TOO_LARGE`,
    /// `LOCATION_MATCHED_NOTHING`, `MODE_CHANGED`, `GITLINK_CHANGED`,
    /// `LFS_POINTER`, `NON_UTF8_CONTENT`.
    pub code: String,
    pub severity: Severity,
    /// Human-readable description (what the UI displays).
    pub message: String,
    /// File path the warning is about, when file-scoped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Concern id the warning is about, when concern-scoped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concern_id: Option<String>,
}

impl Warning {
    pub fn new(code: &str, severity: Severity, message: String) -> Self {
        Warning {
            code: code.to_string(),
            severity,
            message,
            path: None,
            concern_id: None,
        }
    }

    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    pub fn with_concern(mut self, id: impl Into<String>) -> Self {
        self.concern_id = Some(id.into());
        self
    }
}

/// How seriously a warning should be taken.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Notable but expected (e.g. a file type change the reviewer should
    /// glance at).
    Info,
    /// Something was not fully rendered or matched; the reviewer may be
    /// seeing less than the whole change.
    Warning,
}

/// Identifies exactly what a result applies to. Consumers deciding whether
/// to act on a result must compare `head_oid` against the commit they are
/// about to act on — a result for one commit must never be applied to
/// another.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReviewInfo {
    /// Random id of this review session (not the session URL token).
    pub session_id: String,
    /// Version of the ronten binary that produced this result.
    pub ronten_version: String,
    /// The base ref exactly as passed via `--base`.
    pub base_ref: String,
    /// Full commit oid the base ref resolved to at session start. Null only
    /// for sessions without a git repository (`ronten demo`).
    pub base_oid: Option<String>,
    /// Full commit oid `HEAD` resolved to at session start. Submit re-checks
    /// that `HEAD` still resolves to this oid, so a result always describes
    /// this exact commit.
    pub head_oid: Option<String>,
    /// Full oid of `merge-base(base, HEAD)` — the left side of the diff.
    pub merge_base_oid: Option<String>,
    /// SHA-256 (lowercase hex) of the canonical serialization of the diff
    /// the reviewer saw.
    pub diff_sha256: String,
    /// SHA-256 (lowercase hex) of the canonical serialization of the full
    /// concerns input (ids, titles, descriptions, risks, locations).
    pub concerns_sha256: String,
    /// Assurance level of this result. Always `advisory` today: the launching
    /// agent can read the session URL from stderr, so ronten cannot prove the
    /// submit came from a human. Do not use an advisory result as a
    /// security-enforcing approval gate.
    pub assurance: Assurance,
}

/// How much a consumer may trust that a human produced this result.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Assurance {
    /// The result reflects what the review UI submitted, but the launching
    /// agent had access to the session URL and could have submitted itself.
    Advisory,
}

/// Overall decision derived from the per-concern verdicts.
///
/// Abort/timeout exits never emit stdout JSON, so an `Abort` decision was
/// unreachable in the output contract; it is intentionally not a variant
/// here (see `Phase`/`Outcome` for how those exits are represented).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Decision {
    Approve,
    RequestChanges,
}

/// The human's verdict on a single concern, plus any comments left on it.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ConcernResult {
    pub id: String,
    pub verdict: Verdict,
    pub comments: Vec<Comment>,
}

/// Per-concern verdict.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    Approve,
    RequestChanges,
}

/// A comment left on a specific line of the diff.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Comment {
    pub path: String,
    pub side: Side,
    /// 1-based line number on the given side (0 is invalid).
    #[schemars(range(min = 1))]
    pub line: u32,
    /// Comment text (at most 10000 characters; must be non-blank after
    /// trimming whitespace).
    #[schemars(length(min = 1, max = 10000))]
    pub body: String,
}

/// Derive the overall decision from a set of per-concern verdicts.
///
/// Any `RequestChanges` verdict wins and makes the overall decision
/// `RequestChanges`. An empty iterator (or one containing only `Approve`)
/// yields `Approve`.
pub fn derive_decision(verdicts: impl IntoIterator<Item = Verdict>) -> Decision {
    if verdicts
        .into_iter()
        .any(|v| matches!(v, Verdict::RequestChanges))
    {
        Decision::RequestChanges
    } else {
        Decision::Approve
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_spec_example() {
        let json = r#"{
          "version": 1, "summary": "s",
          "concerns": [{ "id": "auth-core", "title": "t", "risk": "high",
            "locations": [ {"path": "a.ts"}, {"path": "b.ts", "side": "new", "start": 10, "end": 42} ] }]
        }"#;
        let input: ConcernsInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.concerns[0].id, "auth-core");
        assert!(matches!(input.concerns[0].risk, Risk::High));
        assert_eq!(input.concerns[0].locations[0].start, None);
        assert_eq!(input.concerns[0].locations[1].end, Some(42));
    }

    #[test]
    fn rejects_unknown_fields_on_v1_input() {
        // version 1 しか受け付けない契約で unknown field を黙って無視すると、
        // 例えば "statr" のような typo が「ファイル全体claim」へ静かに拡大する。
        let top = r#"{"version":1, "bogus":1, "concerns":[{"id":"a","title":"t","risk":"low"}]}"#;
        assert!(serde_json::from_str::<ConcernsInput>(top).is_err());

        let concern =
            r#"{"version":1, "concerns":[{"id":"a","title":"t","risk":"low","extra":1}]}"#;
        assert!(serde_json::from_str::<ConcernsInput>(concern).is_err());

        let location = r#"{"version":1, "concerns":[{"id":"a","title":"t","risk":"low",
            "locations":[{"path":"a.ts","statr":120}]}]}"#;
        assert!(serde_json::from_str::<ConcernsInput>(location).is_err());

        let valid = r#"{"version":1, "concerns":[{"id":"a","title":"t","risk":"low",
            "locations":[{"path":"a.ts","start":120}]}]}"#;
        assert!(serde_json::from_str::<ConcernsInput>(valid).is_ok());
    }

    #[test]
    fn verdict_and_decision_serialize_kebab_case() {
        assert_eq!(
            serde_json::to_string(&Verdict::RequestChanges).unwrap(),
            "\"request-changes\""
        );
        assert_eq!(
            serde_json::to_string(&Decision::Approve).unwrap(),
            "\"approve\""
        );
    }

    #[test]
    fn input_schema_denies_unknown_fields_on_every_object() {
        // Fix 6 regression: `deny_unknown_fields` on ConcernsInput/Concern/
        // Location must keep showing up as `"additionalProperties": false`
        // in the generated JSON Schema. A schemars major bump could silently
        // change how that constraint is rendered (or drop it) without any
        // Rust-level compile error, quietly reopening the "unknown fields
        // widen a location to whole-file" hole `deny_unknown_fields` closes
        // — this pins the schema's *text* so that would fail here first.
        let schema = serde_json::to_string(&schemars::schema_for!(ConcernsInput))
            .expect("schema serializes to JSON");
        let count = schema.matches("\"additionalProperties\":false").count();
        assert!(
            count >= 3,
            "expected additionalProperties:false at least 3 times \
             (ConcernsInput, Concern, Location), found {count} in schema: {schema}"
        );
    }

    #[test]
    fn decision_derivation() {
        use Verdict::*;
        assert!(matches!(
            derive_decision([Approve, Approve]),
            Decision::Approve
        ));
        assert!(matches!(
            derive_decision([Approve, RequestChanges]),
            Decision::RequestChanges
        ));
        assert!(matches!(derive_decision([]), Decision::Approve));
    }
}
