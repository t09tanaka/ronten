//! Data model for the concerns input contract and result output contract.
//!
//! Unknown fields on input types are ignored (no `deny_unknown_fields`) to keep
//! the contract forward-compatible with newer agent-side producers.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Top-level input: a list of concerns an agent wants a human to review.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ConcernsInput {
    pub version: u32,
    #[serde(default)]
    pub summary: Option<String>,
    pub concerns: Vec<Concern>,
}

/// A single concern raised against the diff.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Concern {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    pub risk: Risk,
    #[serde(default)]
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
    #[serde(default)]
    pub start: Option<u32>,
    #[serde(default)]
    pub end: Option<u32>,
}

/// Which side of a diff a location or comment refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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
    pub line: u32,
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
