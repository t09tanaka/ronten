//! In-memory review session state: the draft a human is editing, the
//! immutable diff/mapping/concerns data it's reviewing, and the plumbing to
//! turn a submitted draft into a `ResultOutput`.

use crate::gitdiff::FileDiff;
use crate::mapping::{HunkRef, Mapping, UnmappedLine, UNMAPPED_ID};
use crate::model::{
    derive_decision, Comment, ConcernResult, ConcernsInput, ResultOutput, Risk, Side, Verdict,
    SUPPORTED_VERSION,
};
use crate::server::Outcome;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use tokio::sync::mpsc::Sender;

/// How a session ended. Exactly one terminal state is ever set, via
/// `SessionState::try_finish`, so submit/abort/timeout races resolve to a
/// single winner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Terminal {
    Submitted,
    Aborted,
    TimedOut,
}

/// Maximum number of comments per concern (and of general comments).
const MAX_COMMENTS: usize = 500;
/// Maximum comment body length in characters.
const MAX_COMMENT_CHARS: usize = 10_000;

/// The human's in-progress (or final, at submit time) review state.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Draft {
    #[serde(default)]
    pub concerns: HashMap<String, ConcernDraft>,
    #[serde(default)]
    pub general_comments: Vec<String>,
    /// content が描画されない file（FileDiff::is_opaque）の明示 acknowledge。
    /// 値は session payload の files[] における index。
    #[serde(default)]
    pub acknowledged_opaque: Vec<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConcernDraft {
    #[serde(default)]
    pub verdict: Option<Verdict>,
    #[serde(default)]
    pub comments: Vec<Comment>,
}

/// Everything the UI needs, sent by `GET /api/{token}/session`.
#[derive(Serialize)]
pub struct SessionPayload<'a> {
    pub title: &'a str,
    pub summary: Option<&'a str>,
    pub files: &'a [FileDiff],
    pub concerns: Vec<ConcernView<'a>>,
    pub unmapped_lines: &'a [UnmappedLine],
    pub warnings: &'a [String],
    pub draft: Draft,
    pub submitted: bool,
}

#[derive(Serialize)]
pub struct ConcernView<'a> {
    pub id: &'a str,
    pub title: &'a str,
    pub description: Option<&'a str>,
    pub risk: Option<Risk>,
    pub unmapped: bool,
    pub hunks: &'a [HunkRef],
}

/// All state for one review session, shared behind an `Arc` across handlers.
pub struct SessionState {
    pub title: String,
    pub summary: Option<String>,
    pub files: Vec<FileDiff>,
    pub mapping: Mapping,
    pub input: ConcernsInput,
    pub token: String,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub draft: Mutex<Draft>,
    pub finished: Mutex<Option<Terminal>>,
    pub outcome_tx: Sender<Outcome>,
}

impl SessionState {
    /// Ordered ids that require a verdict before submit can succeed: the
    /// input's concern order, then `_unmapped` if any hunks went unclaimed.
    pub fn required_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.input.concerns.iter().map(|c| c.id.clone()).collect();
        if !self.mapping.unmapped.is_empty() {
            ids.push(UNMAPPED_ID.to_string());
        }
        ids
    }

    /// Atomically claims the session's single terminal state. Returns `true`
    /// if `t` won (the caller owns the outcome) or `false` if another path
    /// already finished the session.
    pub fn try_finish(&self, t: Terminal) -> bool {
        let mut finished = self.finished.lock().unwrap();
        if finished.is_some() {
            false
        } else {
            *finished = Some(t);
            true
        }
    }

    /// The hunks assigned to a concern id (`_unmapped` maps to the
    /// unclaimed bucket). Unknown ids yield an empty slice.
    fn hunk_refs_for(&self, id: &str) -> &[HunkRef] {
        if id == UNMAPPED_ID {
            &self.mapping.unmapped
        } else {
            self.mapping
                .concerns
                .iter()
                .find(|mc| mc.id == id)
                .map(|mc| mc.hunks.as_slice())
                .unwrap_or(&[])
        }
    }

    /// Every `(path, side, line)` a comment on this concern may anchor to,
    /// derived from the diff lines of the concern's assigned hunks. Hunk-less
    /// files (binary/pure rename/too-large) contribute no anchors.
    fn valid_anchors_for(&self, id: &str) -> HashSet<(&str, Side, u32)> {
        let mut anchors = HashSet::new();
        for r in self.hunk_refs_for(id) {
            let file = &self.files[r.file];
            let Some(hi) = r.hunk else { continue };
            for line in &file.hunks[hi].lines {
                if let (Some(no), Some(path)) = (line.old_no, file.old_path.as_deref()) {
                    anchors.insert((path, Side::Old, no));
                }
                if let (Some(no), Some(path)) = (line.new_no, file.new_path.as_deref()) {
                    anchors.insert((path, Side::New, no));
                }
            }
        }
        anchors
    }

    /// Fully validates a draft against the session's contract before submit:
    /// unknown concern ids, comment anchors outside the concern's assigned
    /// hunks, blank or oversized bodies, and comment-count limits. Returns
    /// human-readable violation descriptions (empty = valid). `PUT /draft`
    /// deliberately skips this — the draft is a lenient scratchpad — so
    /// submit must always run the full check.
    pub fn validate_draft(&self, draft: &Draft) -> Vec<String> {
        let mut violations = Vec::new();
        let required: HashSet<String> = self.required_ids().into_iter().collect();
        // Mirrors the frontend's `isVerdictConfirmed` (frontend/src/lib/confirmation.ts):
        // a request-changes verdict needs a reason, either a comment on the
        // concern itself or at least one general comment on the review.
        let has_general_comment = !draft.general_comments.is_empty();

        let mut ids: Vec<&String> = draft.concerns.keys().collect();
        ids.sort();
        for id in ids {
            let cd = &draft.concerns[id];
            if !required.contains(id.as_str()) {
                violations.push(format!("unknown concern id {id:?}"));
                continue;
            }
            if matches!(cd.verdict, Some(Verdict::RequestChanges))
                && cd.comments.is_empty()
                && !has_general_comment
            {
                violations.push(format!(
                    "concern {id:?}: request-changes requires a comment explaining the reason"
                ));
            }
            if cd.comments.len() > MAX_COMMENTS {
                violations.push(format!(
                    "concern {id:?}: too many comments: {} (maximum {MAX_COMMENTS})",
                    cd.comments.len()
                ));
                continue;
            }
            let anchors = self.valid_anchors_for(id);
            for c in &cd.comments {
                let at = format!("{}:{}:{}", c.path, side_name(c.side), c.line);
                if c.body.trim().is_empty() {
                    violations.push(format!("concern {id:?}: blank comment body at {at}"));
                } else if c.body.chars().count() > MAX_COMMENT_CHARS {
                    violations.push(format!(
                        "concern {id:?}: comment body at {at} exceeds {MAX_COMMENT_CHARS} characters"
                    ));
                }
                if !anchors.contains(&(c.path.as_str(), c.side, c.line)) {
                    violations.push(format!(
                        "concern {id:?}: comment anchor {at} does not match any diff line assigned to this concern"
                    ));
                }
            }
        }

        if draft.general_comments.len() > MAX_COMMENTS {
            violations.push(format!(
                "too many general comments: {} (maximum {MAX_COMMENTS})",
                draft.general_comments.len()
            ));
        } else {
            for (i, g) in draft.general_comments.iter().enumerate() {
                if g.trim().is_empty() {
                    violations.push(format!("general comment #{}: blank body", i + 1));
                } else if g.chars().count() > MAX_COMMENT_CHARS {
                    violations.push(format!(
                        "general comment #{}: exceeds {MAX_COMMENT_CHARS} characters",
                        i + 1
                    ));
                }
            }
        }

        let opaque_indices: HashSet<usize> = self
            .files
            .iter()
            .enumerate()
            .filter(|(_, f)| f.is_opaque())
            .map(|(i, _)| i)
            .collect();
        let acknowledged: HashSet<usize> = draft.acknowledged_opaque.iter().copied().collect();
        let mut acked: Vec<usize> = acknowledged.iter().copied().collect();
        acked.sort_unstable();
        for i in acked {
            let Some(file) = self.files.get(i) else {
                violations.push(format!("acknowledged_opaque: unknown file index {i}"));
                continue;
            };
            if !opaque_indices.contains(&i) {
                let path = file
                    .new_path
                    .as_deref()
                    .or(file.old_path.as_deref())
                    .unwrap_or("");
                violations.push(format!("acknowledged_opaque: file {path} is not opaque"));
            }
        }
        let mut missing_ack: Vec<usize> =
            opaque_indices.difference(&acknowledged).copied().collect();
        missing_ack.sort_unstable();
        for i in missing_ack {
            let file = &self.files[i];
            let path = file
                .new_path
                .as_deref()
                .or(file.old_path.as_deref())
                .unwrap_or("");
            violations.push(format!("opaque change not acknowledged: {path}"));
        }

        violations
    }

    /// Builds the final `ResultOutput` from a submitted draft. Callers must
    /// have already verified every `required_ids()` entry has a verdict.
    pub fn build_result(&self, draft: &Draft) -> ResultOutput {
        let required = self.required_ids();
        let concerns: Vec<ConcernResult> = required
            .iter()
            .map(|id| {
                let cd = draft.concerns.get(id);
                ConcernResult {
                    id: id.clone(),
                    verdict: cd
                        .and_then(|c| c.verdict)
                        .expect("caller validated required verdicts before calling build_result"),
                    comments: cd.map(|c| c.comments.clone()).unwrap_or_default(),
                }
            })
            .collect();
        let decision = derive_decision(concerns.iter().map(|c| c.verdict));
        ResultOutput {
            // The contract version ronten processed, not an echo of the
            // input (which validate_concerns already pinned to this value).
            version: SUPPORTED_VERSION,
            decision,
            concerns,
            general_comments: draft.general_comments.clone(),
            warnings: self.mapping.warnings.clone(),
            started_at: self.started_at.to_rfc3339(),
            submitted_at: chrono::Utc::now().to_rfc3339(),
        }
    }
}

/// Lowercase wire name for a side, matching its serde serialization.
fn side_name(side: Side) -> &'static str {
    match side {
        Side::Old => "old",
        Side::New => "new",
    }
}
