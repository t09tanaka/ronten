//! In-memory review session state: the draft a human is editing, the
//! immutable diff/mapping/concerns data it's reviewing, and the plumbing to
//! turn a submitted draft into a `ResultOutput`.

use crate::gitdiff::{AckReason, FileDiff};
use crate::mapping::{HunkRef, Mapping, UnmappedLine, UNMAPPED_ID};
use crate::model::{
    derive_decision, Acknowledgement, Assurance, BuildInfo, Comment, ConcernResult, ConcernsInput,
    FileAudit, ResultOutput, ReviewInfo, Risk, Side, Verdict, Warning, WorktreeAudit,
    OUTPUT_VERSION,
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
/// Maximum comment body length in characters (Unicode scalar values, i.e.
/// `str::chars().count()` — NOT UTF-16 code units, which is what a browser's
/// `String.prototype.length` counts; the frontend must count the same way
/// for its limit to actually match this one, see P1-6).
pub const MAX_COMMENT_CHARS: usize = 10_000;
/// Review-wide cap on the total number of comments across every concern's
/// comments AND the general comments — not just the per-concern/per-general
/// `MAX_COMMENTS` above. This is what actually keeps a draft that satisfies
/// every per-field limit submittable on the wire: per-concern/per-comment
/// caps alone permit far more content than [`crate::server::MAX_BODY_BYTES`]
/// (8 MiB) can carry (200 concerns x 500 comments x 10,000 chars is nowhere
/// near submittable) — see P1-6.
pub const MAX_TOTAL_COMMENTS: usize = 1000;
/// Review-wide cap on the summed character length (Unicode scalars, same
/// counting as [`MAX_COMMENT_CHARS`]) of every comment body — concern
/// comments plus general comments together. Sized so a draft at every
/// advertised limit still serializes under
/// [`crate::server::MAX_BODY_BYTES`]: even at the UTF-8 worst case of 4
/// bytes/scalar this is 6 MiB, leaving headroom under the 8 MiB body cap for
/// JSON structure, paths, and anchors (see the `limits_are_wire_consistent`
/// test in server.rs).
pub const MAX_TOTAL_COMMENT_CHARS: usize = 1_500_000;

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
    /// See [`MAX_TOTAL_COMMENTS`].
    pub max_total_comments: usize,
    /// See [`MAX_TOTAL_COMMENT_CHARS`].
    pub max_total_comment_chars: usize,
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

/// Why `file`'s content was not rendered to the reviewer, when it wasn't —
/// `None` means it was (an ordinary text change, or a hunk-less change with
/// nothing to hide, e.g. a pure rename). A file can trip more than one of
/// these (e.g. an LFS pointer that also fails UTF-8 decoding would not, but
/// `content_kind` opacity and an LFS pointer both being true is
/// contradictory by construction — LFS pointer text is always valid UTF-8);
/// this picks a single dominant reason, checked in the order content
/// opacity (the diff body itself couldn't show it) then LFS pointer (the
/// diff body shows a pointer, not real data) then submodule (an existing
/// gitlink's pointer moved — the nested diff is never shown).
fn omission_reason(file: &FileDiff) -> Option<&'static str> {
    match file.content_kind {
        crate::gitdiff::ContentKind::Binary => return Some("binary"),
        crate::gitdiff::ContentKind::NonUtf8 => return Some("non_utf8"),
        crate::gitdiff::ContentKind::TooLarge => return Some("too_large"),
        crate::gitdiff::ContentKind::Text => {}
    }
    if file.lfs_pointer {
        return Some("lfs_pointer");
    }
    let gitlink_involved = file.old_type == Some(crate::gitdiff::FileType::Gitlink)
        || file.new_type == Some(crate::gitdiff::FileType::Gitlink);
    if gitlink_involved && file.old_oid != file.new_oid {
        return Some("submodule");
    }
    None
}

impl From<&FileDiff> for FileAudit {
    fn from(file: &FileDiff) -> Self {
        let omission_reason = omission_reason(file);
        FileAudit {
            file_id: file.id(),
            old_path: file.old_path.clone(),
            new_path: file.new_path.clone(),
            old_mode: file.old_mode.clone(),
            new_mode: file.new_mode.clone(),
            file_type: file.new_type.or(file.old_type),
            old_oid: file.old_oid.clone(),
            new_oid: file.new_oid.clone(),
            content_kind: file.content_kind,
            rendered: omission_reason.is_none(),
            omission_reason: omission_reason.map(str::to_string),
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

/// Worktree cleanliness captured once at session start (in `review::run`,
/// or left at defaults for sessions with no dirty gate at all — demo, or
/// `--dirty-policy ignore`). Kept separate from `model::WorktreeAudit`
/// because the submit-time half of that audit
/// (`checked_at_submit`/`clean_at_submit`) is only known once `build_result`
/// actually runs the re-check, never at session construction.
#[derive(Debug, Clone, Default)]
pub struct WorktreeStartAudit {
    /// Whether the worktree was actually queried at start. `false` under
    /// `--dirty-policy ignore`, or when the query itself failed (a `Warn`
    /// policy proceeds on git failure without ever getting a status).
    pub checked: bool,
    /// Meaningful only when `checked` is `true`.
    pub clean: bool,
    pub excluded_paths: Vec<String>,
}

/// Result of the submit-time worktree re-check `build_result` embeds in the
/// result's `WorktreeAudit` — computed by the caller (an async context; the
/// re-check itself needs to shell out to git) and handed in, since
/// `build_result` itself stays synchronous.
#[derive(Debug, Clone, Default)]
pub struct WorktreeSubmitAudit {
    /// Whether the re-check actually ran and succeeded.
    pub checked: bool,
    /// Meaningful only when `checked` is `true`.
    pub clean: bool,
    /// Set when the re-check was attempted but failed (git error): recorded
    /// as a warning in the result rather than blocking the submit — the
    /// approval itself is already protected by the `HEAD` pin, and worktree
    /// cleanliness here is audit information, not a gate.
    pub warning: Option<Warning>,
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
    /// Wire name of the `--dirty-policy` in effect (`"error"`, `"warn"`, or
    /// `"ignore"`), carried straight into the result's `WorktreeAudit`.
    /// `"ignore"` for sessions with no dirty-worktree concept at all (demo).
    pub dirty_policy: String,
    /// Worktree cleanliness as captured once at session start; see
    /// [`WorktreeStartAudit`].
    pub worktree_start: WorktreeStartAudit,
    /// Repo-relative path exempted from the dirty check (the concerns input
    /// file, when present untracked at its own path) — re-applied at the
    /// submit-time re-check so the audit is consistent between start and
    /// submit. `None` when no exemption applies (including demo/no-repo
    /// sessions).
    pub worktree_exempt_path: Option<String>,
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

    /// Resource-only violations across the whole draft: per-concern and
    /// per-general comment counts (`MAX_COMMENTS`), per-comment body length
    /// (`MAX_COMMENT_CHARS`), and the two review-wide totals
    /// (`MAX_TOTAL_COMMENTS`/`MAX_TOTAL_COMMENT_CHARS`). Deliberately
    /// excludes everything [`validate_draft`](Self::validate_draft)
    /// additionally checks — blank bodies, anchor validity, unknown concern
    /// ids, acknowledgements — those are meaningful only at submit time.
    /// `PUT /draft` runs only this narrower check, so the draft stays a
    /// lenient scratchpad while still being kept truthfully within the
    /// limits advertised to the UI (and therefore always submittable on the
    /// wire — see P1-6).
    pub fn resource_cap_violations(&self, draft: &Draft) -> Vec<String> {
        let mut violations = Vec::new();
        let mut total_comments = 0usize;
        let mut total_chars = 0usize;

        let mut ids: Vec<&String> = draft.concerns.keys().collect();
        ids.sort();
        for id in ids {
            let cd = &draft.concerns[id];
            total_comments += cd.comments.len();
            if cd.comments.len() > MAX_COMMENTS {
                violations.push(format!(
                    "concern {id:?}: too many comments: {} (maximum {MAX_COMMENTS})",
                    cd.comments.len()
                ));
            }
            for c in &cd.comments {
                let n = c.body.chars().count();
                total_chars += n;
                if n > MAX_COMMENT_CHARS {
                    violations.push(format!(
                        "concern {id:?}: comment body at {}:{}:{} exceeds {MAX_COMMENT_CHARS} characters",
                        c.path, side_name(c.side), c.line
                    ));
                }
            }
        }

        total_comments += draft.general_comments.len();
        if draft.general_comments.len() > MAX_COMMENTS {
            violations.push(format!(
                "too many general comments: {} (maximum {MAX_COMMENTS})",
                draft.general_comments.len()
            ));
        }
        for (i, g) in draft.general_comments.iter().enumerate() {
            let n = g.chars().count();
            total_chars += n;
            if n > MAX_COMMENT_CHARS {
                violations.push(format!(
                    "general comment #{}: exceeds {MAX_COMMENT_CHARS} characters",
                    i + 1
                ));
            }
        }

        if total_comments > MAX_TOTAL_COMMENTS {
            violations.push(format!(
                "too many comments across the review: {total_comments} (maximum {MAX_TOTAL_COMMENTS})"
            ));
        }
        if total_chars > MAX_TOTAL_COMMENT_CHARS {
            violations.push(format!(
                "too many comment characters across the review: {total_chars} (maximum {MAX_TOTAL_COMMENT_CHARS})"
            ));
        }

        violations
    }

    /// Fully validates a draft against the session's contract before submit:
    /// unknown concern ids, comment anchors outside the concern's assigned
    /// hunks, blank bodies, and the resource caps from
    /// [`resource_cap_violations`](Self::resource_cap_violations). Returns
    /// human-readable violation descriptions (empty = valid). `PUT /draft`
    /// runs only the narrower resource check — the draft is otherwise a
    /// lenient scratchpad — so submit must always run this full check.
    pub fn validate_draft(&self, draft: &Draft) -> Vec<String> {
        let mut violations = self.resource_cap_violations(draft);
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
                // Already recorded by resource_cap_violations above; skip
                // the (potentially huge) anchor/blank pass below for this
                // concern.
                continue;
            }
            let anchors = self.valid_anchors_for(id);
            for c in &cd.comments {
                let at = format!("{}:{}:{}", c.path, side_name(c.side), c.line);
                if c.body.trim().is_empty() {
                    violations.push(format!("concern {id:?}: blank comment body at {at}"));
                }
                if !anchors.contains(&(c.path.as_str(), c.side, c.line)) {
                    violations.push(format!(
                        "concern {id:?}: comment anchor {at} does not match any diff line assigned to this concern"
                    ));
                }
            }
        }

        if draft.general_comments.len() <= MAX_COMMENTS {
            for (i, g) in draft.general_comments.iter().enumerate() {
                if g.trim().is_empty() {
                    violations.push(format!("general comment #{}: blank body", i + 1));
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
    /// have already verified every `required_ids()` entry has a verdict, and
    /// every `draft.acknowledgements` entry is a known, ack-required file id
    /// (both enforced by [`validate_draft`](Self::validate_draft)).
    ///
    /// `worktree_submit` is the submit-time worktree re-check — computed by
    /// the caller (an async context able to shell out to git) and handed in
    /// so this method itself stays synchronous.
    pub fn build_result(
        &self,
        draft: &Draft,
        worktree_submit: WorktreeSubmitAudit,
    ) -> ResultOutput {
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

        let submitted_at = chrono::Utc::now();
        let submitted_at_rfc3339 = submitted_at.to_rfc3339();

        let files: Vec<FileAudit> = self.files.iter().map(FileAudit::from).collect();

        // File order, not draft order (the draft's `acknowledgements` is
        // effectively an unordered set on the wire) — deterministic and
        // matches `files` above. Acknowledgement timestamps are not tracked
        // per-ack (the draft is a scratchpad edited freely until submit);
        // recording the submit instant is the only value that can't go
        // stale relative to when the acknowledgement became final.
        let acked: HashSet<&String> = draft.acknowledgements.iter().collect();
        let acknowledgements: Vec<Acknowledgement> = self
            .files
            .iter()
            .filter(|f| acked.contains(&f.id()))
            .map(|f| Acknowledgement {
                file_id: f.id(),
                reasons: f.ack_reasons(),
                acknowledged_at: submitted_at_rfc3339.clone(),
            })
            .collect();

        let mut warnings = self.mapping.warnings.clone();
        if let Some(w) = worktree_submit.warning {
            warnings.push(w);
        }

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
            warnings,
            files,
            acknowledgements,
            worktree: WorktreeAudit {
                policy: self.dirty_policy.clone(),
                checked_at_start: self.worktree_start.checked,
                clean_at_start: self.worktree_start.clean,
                checked_at_submit: worktree_submit.checked,
                clean_at_submit: worktree_submit.clean,
                excluded_paths: self.worktree_start.excluded_paths.clone(),
            },
            build: BuildInfo::current(),
            started_at: self.started_at.to_rfc3339(),
            submitted_at: submitted_at_rfc3339,
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
