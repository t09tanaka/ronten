//! Data model for the concerns input contract and result output contract.
//!
//! Unknown fields on input types are ignored (no `deny_unknown_fields`) to keep
//! the contract forward-compatible with newer agent-side producers.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The only concerns/result contract version this build of ronten accepts
/// (and the version it stamps on every emitted result).
pub const SUPPORTED_VERSION: u32 = 1;

/// Top-level input: a list of concerns an agent wants a human to review.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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

/// Top-level output: the human's decision plus per-concern verdicts.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ResultOutput {
    pub version: u32,
    pub decision: Decision,
    pub concerns: Vec<ConcernResult>,
    pub general_comments: Vec<String>,
    pub warnings: Vec<String>,
    pub started_at: String,
    pub submitted_at: String,
}

/// Overall decision derived from the per-concern verdicts.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Decision {
    Approve,
    RequestChanges,
    Abort,
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
    Comment,
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
/// `RequestChanges`. `Comment` is non-blocking. An empty iterator (or one
/// containing only `Approve`/`Comment`) yields `Approve`.
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
    fn parses_spec_example_and_ignores_unknown_fields() {
        let json = r#"{
          "version": 1, "summary": "s", "extra_field": true,
          "concerns": [{ "id": "auth-core", "title": "t", "risk": "high",
            "unknown": 1,
            "locations": [ {"path": "a.ts"}, {"path": "b.ts", "side": "new", "start": 10, "end": 42} ] }]
        }"#;
        let input: ConcernsInput = serde_json::from_str(json).unwrap();
        assert_eq!(input.concerns[0].id, "auth-core");
        assert!(matches!(input.concerns[0].risk, Risk::High));
        assert_eq!(input.concerns[0].locations[0].start, None);
        assert_eq!(input.concerns[0].locations[1].end, Some(42));
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
    fn decision_derivation() {
        use Verdict::*;
        assert!(matches!(
            derive_decision([Approve, Comment]),
            Decision::Approve
        ));
        assert!(matches!(
            derive_decision([Approve, RequestChanges]),
            Decision::RequestChanges
        ));
        assert!(matches!(derive_decision([]), Decision::Approve));
    }
}
