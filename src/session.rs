//! In-memory review session state: the draft a human is editing, the
//! immutable diff/mapping/concerns data it's reviewing, and the plumbing to
//! turn a submitted draft into a `ResultOutput`.

use crate::gitdiff::{AckReason, FileDiff};
use crate::mapping::{HunkRef, Mapping, UnmappedLine, UNMAPPED_ID};
use crate::model::{
    derive_decision, Assurance, Comment, ConcernResult, ConcernsInput, ResultOutput, ReviewInfo,
    Risk, Side, Verdict, Warning, OUTPUT_VERSION,
};
use crate::server::Outcome;
use crate::snapshot::ReviewSnapshot;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, MutexGuard, PoisonError};

/// Locks a mutex, recovering the guard if a previous holder panicked. The
/// data behind every session mutex stays structurally valid even when a
/// panicking handler unwound mid-update (worst case: a stale draft), and
/// propagating the poison would instead turn one panicked request into a
/// permanently wedged session — every later lock would panic too, including
/// the one the outcome waiter needs to finish the process.
pub(crate) fn lock_ignore_poison<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Where a session is in its lifecycle. The terminal claim, the outcome it
/// resolved to, AND the editable draft all live in one value behind one
/// mutex, so claiming the terminal state, publishing the outcome, and
/// freezing the draft are a single atomic step — there is no window where
/// the session is finished but its outcome is unreadable, and no window
/// where a save can slip past a finished check and rewrite a frozen draft.
#[derive(Debug)]
pub enum Phase {
    Reviewing(DraftSlot),
    /// The second field is the `mutation_id` of the submit that drove this
    /// session to `Finished` — `None` for an abort/timeout, since those
    /// aren't idempotency-tracked. A repeat submit presenting this same id
    /// is a lost-response retry, not a new attempt; see
    /// [`try_finish_at_revision`](SessionState::try_finish_at_revision).
    Finished(Outcome, Option<String>),
}

/// Maximum number of comments per concern (and of general comments).
pub const MAX_COMMENTS: usize = 500;
/// Maximum comment body length in characters.
pub const MAX_COMMENT_CHARS: usize = 10_000;

/// The draft plus its monotonically increasing revision. Every accepted
/// `PUT /draft` must present the current revision and bumps it by one, so
/// two tabs editing the same session cannot silently overwrite each other:
/// the stale tab's save is refused with a conflict instead.
#[derive(Debug, Default)]
pub struct DraftSlot {
    pub draft: Draft,
    pub revision: u64,
    /// The `(mutation_id, revision)` of the last accepted `PUT /draft` — the
    /// revision it produced, not the one it was submitted with. A repeat
    /// save presenting this same `mutation_id` is a lost-response retry: it
    /// replays this recorded revision instead of re-applying (or 409ing on
    /// a now-stale `revision` field it may carry).
    pub last_save: Option<(String, u64)>,
}

/// Validation limits the server enforces, sent to the UI in the session
/// payload so input fields can enforce the same bounds client-side instead
/// of hardcoding a mirror of these numbers.
#[derive(Serialize)]
pub struct Limits {
    pub max_comments: usize,
    pub max_comment_chars: usize,
    pub max_draft_bytes: usize,
}

/// The human's in-progress (or final, at submit time) review state.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Draft {
    #[serde(default)]
    pub concerns: HashMap<String, ConcernDraft>,
    #[serde(default)]
    pub general_comments: Vec<String>,
    /// `FileDiff::id()` of every file the reviewer explicitly acknowledged
    /// — required for every file whose `FileDiff::ack_reasons()` is
    /// non-empty (opaque content, gitlink pointer change, mode change, an
    /// added/deleted symlink, a new executable, a regular-to-symlink type
    /// change, an LFS pointer — see `AckReason`). ID-based rather than
    /// index-based: an index into `files[]` is fragile (a stale client, or
    /// the file list being rebuilt, could silently point the ack at the
    /// wrong file); a stable id derived from the file's own identity cannot.
    #[serde(default)]
    pub acknowledgements: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConcernDraft {
    #[serde(default)]
    pub verdict: Option<Verdict>,
    #[serde(default)]
    pub comments: Vec<Comment>,
}

/// Wire view of one file's diff, plus the server-computed ack requirement
/// for it (see `FileDiff::ack_reasons`). Flattens `FileDiff`'s own fields
/// together with `id`/`ack_required`/`ack_reasons` so the frontend gets one
/// flat object per file; it reads `ack_required`/`ack_reasons` here rather
/// than recomputing the policy itself — the duplication that used to drift
/// between Rust and TypeScript (P0-5).
#[derive(Serialize)]
pub struct FileView<'a> {
    #[serde(flatten)]
    pub file: &'a FileDiff,
    pub id: String,
    pub ack_required: bool,
    pub ack_reasons: Vec<AckReason>,
}

impl<'a> From<&'a FileDiff> for FileView<'a> {
    fn from(file: &'a FileDiff) -> Self {
        let ack_reasons = file.ack_reasons();
        FileView {
            id: file.id(),
            ack_required: !ack_reasons.is_empty(),
            ack_reasons,
            file,
        }
    }
}

/// Everything the UI needs, sent by `GET /api/{token}/session`.
#[derive(Serialize)]
pub struct SessionPayload<'a> {
    pub title: &'a str,
    pub summary: Option<&'a str>,
    pub files: Vec<FileView<'a>>,
    pub concerns: Vec<ConcernView<'a>>,
    pub unmapped_lines: &'a [UnmappedLine],
    pub warnings: &'a [Warning],
    pub draft: Draft,
    /// Current draft revision; `PUT /draft` must echo it back.
    pub draft_revision: u64,
    pub limits: Limits,
    /// `null` while the review is open; otherwise how it ended
    /// (`"submitted"` / `"aborted"` / `"timeout"`), so the UI can show the
    /// right terminal screen instead of calling every ending "submitted".
    pub finished: Option<&'static str>,
    /// RFC3339 UTC instant this session's `--timeout` will elapse, so the UI
    /// can render a countdown. `null` when no `--timeout` was given.
    pub deadline_at: Option<String>,
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
    /// Non-secret random id identifying this session in the result JSON
    /// (deliberately distinct from `token`, which grants session access).
    pub session_id: String,
    /// What this session is reviewing, captured once at start.
    pub snapshot: ReviewSnapshot,
    /// Repo root for the submit-time `HEAD` freshness re-check. `None` for
    /// sessions with no git repository behind them (demo), which skips the
    /// check.
    pub repo_root: Option<std::path::PathBuf>,
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// `started_at` + `--timeout`, if one was given; `None` for an
    /// untimed session. Computed once at session construction so every
    /// `GET /session` response reports the same deadline instead of
    /// re-deriving it from an elapsed-time calculation that could drift.
    pub deadline_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Lifecycle phase; see [`Phase`]. Transitioned to `Finished` only by
    /// [`try_finish`]; the in-progress draft lives inside `Reviewing`.
    ///
    /// [`try_finish`]: SessionState::try_finish
    pub phase: Mutex<Phase>,
    /// Pure wake-up signal fired after `phase` transitions to `Finished`.
    /// Deliberately carries no data: the outcome is read back from `phase`,
    /// so the channel cannot disagree with the state and a receiver that
    /// missed a notification still finds the outcome by reading `phase`.
    pub outcome_tx: tokio::sync::watch::Sender<()>,
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

    /// Atomically claims the session's single terminal state *and* publishes
    /// its outcome, then wakes the outcome waiter. Returns `true` if
    /// `outcome` won or `false` if another path already finished the session
    /// — in which case the winner's outcome is already readable via
    /// [`finished_outcome`](Self::finished_outcome), because the claim and
    /// the publish happen under one lock.
    pub fn try_finish(&self, outcome: Outcome) -> bool {
        let mut phase = lock_ignore_poison(&self.phase);
        if matches!(*phase, Phase::Finished(_, _)) {
            return false;
        }
        // Dropping the DraftSlot here freezes the draft: with slot and
        // terminal state behind the same lock, no save can interleave
        // between a finished check and a write. `None`: this path (abort/
        // timeout) has no client-supplied mutation to track for replay.
        *phase = Phase::Finished(outcome, None);
        drop(phase);
        // `send_replace` succeeds regardless of receiver liveness; and even
        // if the waiter misses this wake-up entirely, it reads the outcome
        // from `phase`, never from the channel.
        self.outcome_tx.send_replace(());
        true
    }

    /// The session's outcome, if it has finished.
    pub fn finished_outcome(&self) -> Option<Outcome> {
        match &*lock_ignore_poison(&self.phase) {
            Phase::Reviewing(_) => None,
            Phase::Finished(outcome, _) => Some(outcome.clone()),
        }
    }

    /// Like [`try_finish`](Self::try_finish), but only wins if the caller's
    /// draft revision is still current — all under the same single lock, so
    /// neither another finish nor a concurrent save can interleave. This is
    /// what keeps a stale tab's submit from silently discarding a newer
    /// draft another tab saved.
    ///
    /// `mutation_id` is the id of the submit attempting to finish the
    /// session. If the session is already `Finished(Submitted, _)` with this
    /// exact same id recorded, this is a lost-response retry of the submit
    /// that already won — it returns [`FinishAttempt::AlreadySubmittedSame`]
    /// instead of the usual "session finished" conflict, and does not touch
    /// `phase` again (the already-published outcome stands unchanged).
    pub fn try_finish_at_revision(
        &self,
        outcome: Outcome,
        revision: u64,
        mutation_id: &str,
    ) -> FinishAttempt {
        let mut phase = lock_ignore_poison(&self.phase);
        match &*phase {
            Phase::Finished(o, submitted_id) => {
                if matches!(o, Outcome::Submitted(_))
                    && submitted_id.as_deref() == Some(mutation_id)
                {
                    return FinishAttempt::AlreadySubmittedSame;
                }
                FinishAttempt::AlreadyFinished(outcome_kind(o))
            }
            Phase::Reviewing(slot) => {
                if slot.revision != revision {
                    return FinishAttempt::RevisionConflict(slot.revision);
                }
                *phase = Phase::Finished(outcome, Some(mutation_id.to_string()));
                drop(phase);
                self.outcome_tx.send_replace(());
                FinishAttempt::Won
            }
        }
    }

    /// Wire name of the terminal state, if any.
    pub fn finished_kind(&self) -> Option<&'static str> {
        match &*lock_ignore_poison(&self.phase) {
            Phase::Reviewing(_) => None,
            Phase::Finished(outcome, _) => Some(outcome_kind(outcome)),
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

        // Id-keyed, not index-keyed (P0-5): a file's identity survives the
        // diff being rebuilt or reordered, an array index does not.
        let id_lookup: HashMap<String, &FileDiff> =
            self.files.iter().map(|f| (f.id(), f)).collect();
        let ack_required_ids: HashSet<String> = self
            .files
            .iter()
            .filter(|f| f.ack_required())
            .map(FileDiff::id)
            .collect();
        let acknowledged: HashSet<String> = draft.acknowledgements.iter().cloned().collect();
        let mut acked: Vec<&String> = acknowledged.iter().collect();
        acked.sort();
        for id in acked {
            let Some(file) = id_lookup.get(id) else {
                violations.push(format!("acknowledgements: unknown file id {id:?}"));
                continue;
            };
            if !ack_required_ids.contains(id) {
                let path = file
                    .new_path
                    .as_deref()
                    .or(file.old_path.as_deref())
                    .unwrap_or("");
                violations.push(format!(
                    "acknowledgements: file {path} does not require acknowledgement"
                ));
            }
        }
        let mut missing_ack: Vec<&String> = ack_required_ids.difference(&acknowledged).collect();
        missing_ack.sort();
        for id in missing_ack {
            let file = id_lookup[id];
            let path = file
                .new_path
                .as_deref()
                .or(file.old_path.as_deref())
                .unwrap_or("");
            violations.push(format!("change not acknowledged: {path}"));
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
            version: OUTPUT_VERSION,
            review: ReviewInfo {
                session_id: self.session_id.clone(),
                ronten_version: env!("CARGO_PKG_VERSION").to_string(),
                base_ref: self.snapshot.base_ref.clone(),
                base_oid: self.snapshot.base_oid.clone(),
                head_oid: self.snapshot.head_oid.clone(),
                merge_base_oid: self.snapshot.merge_base_oid.clone(),
                diff_sha256: self.snapshot.diff_sha256.clone(),
                concerns_sha256: self.snapshot.concerns_sha256.clone(),
                assurance: Assurance::Advisory,
            },
            decision,
            concerns,
            general_comments: draft.general_comments.clone(),
            warnings: self.mapping.warnings.clone(),
            started_at: self.started_at.to_rfc3339(),
            submitted_at: chrono::Utc::now().to_rfc3339(),
        }
    }
}

/// Outcome of [`SessionState::try_finish_at_revision`].
#[derive(Debug)]
pub enum FinishAttempt {
    /// The caller claimed the terminal state.
    Won,
    /// Another path already finished the session (wire name of how).
    AlreadyFinished(&'static str),
    /// The session is already `Finished(Submitted, _)` with the exact same
    /// `mutation_id` recorded — a lost-response retry of the winning
    /// submit, not a new attempt. The caller should answer 200, same as
    /// `Won`.
    AlreadySubmittedSame,
    /// The caller's draft revision is stale; the current revision is given.
    RevisionConflict(u64),
}

/// Wire name of an outcome, used in the session payload's `finished` field
/// and in "session finished" conflict responses.
pub fn outcome_kind(outcome: &Outcome) -> &'static str {
    match outcome {
        Outcome::Submitted(_) => "submitted",
        Outcome::Aborted => "aborted",
        Outcome::Timeout => "timeout",
    }
}

/// Lowercase wire name for a side, matching its serde serialization.
fn side_name(side: Side) -> &'static str {
    match side {
        Side::Old => "old",
        Side::New => "new",
    }
}
