//! Token-protected localhost HTTP server exposing the review session.

use crate::assets;
use crate::model::ResultOutput;
use crate::session::{Draft, SessionPayload, SessionState};
use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use rand::Rng;
use serde_json::json;
use std::sync::Arc;

/// `Content-Security-Policy` applied to every non-asset response (index +
/// API JSON). `style-src` allows `'unsafe-inline'` because Svelte's `style:`
/// directive (and similar bound-style bindings) compiles down to inline
/// `style="..."` attributes on elements — there is no build-time way to hash
/// or nonce those, so a strict `style-src 'self'` would break the UI.
/// `font-src` allows `data:` because `app.css` embeds the "論" seal glyph
/// (unicode-range U+8AD6) as an inline `data:font/woff2;base64` subset — with
/// only `default-src 'self'` this glyph's `document.fonts.load` fails with a
/// network error (confirmed against a live build). Every other directive
/// stays locked to `'self'`/`'none'`.
const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; font-src 'self' data:; connect-src 'self'; img-src 'self' data:; frame-ancestors 'none'; base-uri 'none'";

/// Sets response headers that matter because the session token lives in the
/// URL path: caching or leaking it (via history, shared caches, referrers, or
/// content sniffing) would leak session access.
///
/// - Every response gets `Referrer-Policy: no-referrer` and
///   `X-Content-Type-Options: nosniff`.
/// - `/assets/*` (Vite's content-hashed build output) is safe to cache
///   forever.
/// - Everything else (index / API) must never be cached and gets a strict CSP.
async fn security_headers(req: Request, next: Next) -> Response {
    // Prefix-based, so a 404 for a nonexistent file under `/assets/` (e.g. a
    // typo'd hash) still gets the long-lived immutable cache header below
    // instead of `no-store`. That's fine: unlike `/r/{token}` and
    // `/api/{token}/*`, no session token ever appears anywhere in an assets
    // path, so there is nothing sensitive to leak into a shared/browser
    // cache by over-caching a miss.
    let is_asset = req.uri().path().starts_with("/assets/");
    let mut res = next.run(req).await;
    let headers = res.headers_mut();
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    if is_asset {
        headers.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=31536000, immutable"),
        );
    } else {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        headers.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(CONTENT_SECURITY_POLICY),
        );
    }
    res
}

/// What a session ended with; stored in `SessionState::phase` by the single
/// `try_finish` winner. `Timeout` is never produced by the HTTP handlers
/// here — it's claimed by the review loop's `select!` arm when the deadline
/// fires before a submit/abort arrives. `Clone` so losers of the terminal
/// race (and the outcome waiter) can read the winner's outcome back out of
/// the shared phase.
#[derive(Debug, Clone)]
pub enum Outcome {
    Submitted(Box<ResultOutput>),
    Aborted,
    Timeout,
}

/// Hard ceiling on how long any single request may take end-to-end
/// (extraction through handler). Generous compared to every real handler
/// (the slowest does one local git `rev-parse`), so it only ever fires on a
/// wedged handler or a client trickling its request body — either of which
/// would otherwise hold a connection open indefinitely and, after an
/// outcome, stall graceful shutdown until the shutdown deadline kills it.
pub const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Deadline for the single `rev-parse HEAD` `check_head_freshness` runs on
/// the submit path. Timeout hierarchy for a submit must be internal deadline
/// < server request timeout < client timeout — a wedged rev-parse here has
/// to fail well before [`REQUEST_TIMEOUT`] (30s), which in turn must resolve
/// before the frontend's own fetch timeout (40s), so a slow submit is never
/// silently killed by the client while the server is still committing it
/// (the ambiguous-completion bug this hierarchy exists to close). The 60s
/// [`crate::gitdiff::GIT_TIMEOUT`] default is intentionally NOT reused here —
/// it's sized for the initial (potentially large) diff computation, not this
/// single cheap rev-parse.
const HEAD_FRESHNESS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Middleware bounding each request by the timeout passed as state (the
/// production router uses [`REQUEST_TIMEOUT`]). A request that overruns gets
/// `408 Request Timeout` instead of hanging its connection.
async fn request_timeout(
    State(limit): State<std::time::Duration>,
    req: Request,
    next: Next,
) -> Response {
    match tokio::time::timeout(limit, next.run(req)).await {
        Ok(res) => res,
        Err(_) => (
            StatusCode::REQUEST_TIMEOUT,
            Json(json!({"error": "request timed out"})),
        )
            .into_response(),
    }
}

/// Explicit request-body cap for every route (draft saves and submits are
/// the only bodies). This — not axum's implicit default — is the wire
/// contract, and it matches the concerns-input cap: a draft cannot
/// legitimately outgrow the review it annotates. Application-level per-field
/// limits (comment length/count) are far below this; the byte cap exists so
/// the extractor bound and the documented contract are the same number.
pub const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

/// Converts the bare 413 the body-limit extractor produces into the same
/// JSON error shape every other refusal uses, so clients never need a
/// second error-decoding path.
async fn json_payload_too_large(req: Request, next: Next) -> Response {
    let res = next.run(req).await;
    if res.status() == StatusCode::PAYLOAD_TOO_LARGE {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(json!({
                "error": "payload too large",
                "details": [format!("request body exceeds {MAX_BODY_BYTES} bytes")],
            })),
        )
            .into_response();
    }
    res
}

/// 16 random bytes, rendered as lowercase hex — used as the session's
/// unguessable URL token.
pub fn new_token() -> String {
    let mut bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, Body::empty()).into_response()
}

async fn get_index(State(state): State<Arc<SessionState>>, Path(token): Path<String>) -> Response {
    if token != state.token {
        return not_found();
    }
    assets::serve("index.html")
}

async fn get_asset(Path(path): Path<String>) -> Response {
    // `path` is the wildcard suffix captured after `/assets/` (e.g.
    // `index-BFyEZ69R.js`), but rust-embed keys files under the `assets/`
    // prefix they live at in `frontend/dist` on disk, so the lookup key must
    // be re-prefixed to match.
    assets::serve(&format!("assets/{path}"))
}

async fn get_session(
    State(state): State<Arc<SessionState>>,
    Path(token): Path<String>,
) -> Response {
    if token != state.token {
        return not_found();
    }

    let mut concerns: Vec<crate::session::ConcernView> = state
        .input
        .concerns
        .iter()
        .zip(state.mapping.concerns.iter())
        .map(|(c, mc)| crate::session::ConcernView {
            id: &mc.id,
            title: &c.title,
            description: c.description.as_deref(),
            risk: Some(c.risk.clone()),
            unmapped: false,
            hunks: &mc.hunks,
        })
        .collect();
    if !state.mapping.unmapped.is_empty() {
        concerns.push(crate::session::ConcernView {
            id: crate::mapping::UNMAPPED_ID,
            title: "Unmapped changes",
            description: Some(
                "Changes the agent did not assign to any concern. Review them with extra care.",
            ),
            risk: None,
            unmapped: true,
            hunks: &state.mapping.unmapped,
        });
    }

    // One phase read yields draft, revision, and terminal state together,
    // so the payload can never pair a live draft with a finished flag.
    let (draft, draft_revision, finished) = {
        let phase = crate::session::lock_ignore_poison(&state.phase);
        match &*phase {
            crate::session::Phase::Reviewing(slot) => (slot.draft.clone(), slot.revision, None),
            crate::session::Phase::Finished(outcome, _) => (
                Draft::default(),
                0,
                Some(crate::session::outcome_kind(outcome)),
            ),
        }
    };
    let payload = SessionPayload {
        title: &state.title,
        summary: state.summary.as_deref(),
        files: &state.files,
        concerns,
        unmapped_lines: &state.mapping.unmapped_lines,
        warnings: &state.mapping.warnings,
        draft,
        draft_revision,
        limits: crate::session::Limits {
            max_comments: crate::session::MAX_COMMENTS,
            max_comment_chars: crate::session::MAX_COMMENT_CHARS,
            max_draft_bytes: MAX_BODY_BYTES,
        },
        finished,
        deadline_at: state.deadline_at.map(|d| d.to_rfc3339()),
    };
    Json(payload).into_response()
}

/// Wire shape of `PUT /draft` and `POST /submit`: the draft, the revision
/// the client believes is current, and a client-generated id naming this
/// specific mutation. A revision mismatch means another tab (or an older
/// copy of this one) saved in between; accepting the write would silently
/// discard that save, so it is refused with 409 instead. `mutation_id` is
/// what makes a lost-response retry of the SAME mutation safe: replaying it
/// answers with the already-applied result instead of re-applying or
/// 409ing on a revision the retry never learned advanced.
#[derive(serde::Deserialize)]
struct DraftPut {
    revision: u64,
    draft: Draft,
    mutation_id: String,
}

/// Generous ceiling for a client-supplied mutation id (a UUID is 36 chars);
/// this only exists to keep a malformed/hostile id from being stored
/// indefinitely, not to constrain any real client's id format.
const MAX_MUTATION_ID_CHARS: usize = 100;

/// Rejects an empty or oversized `mutation_id` with the same JSON error
/// shape every other refusal uses. `None` means the id is fine.
fn validate_mutation_id(id: &str) -> Option<Response> {
    if id.is_empty() || id.chars().count() > MAX_MUTATION_ID_CHARS {
        return Some(
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({
                    "error": "invalid mutation id",
                    "details": [format!(
                        "mutation_id must be 1-{MAX_MUTATION_ID_CHARS} characters"
                    )],
                })),
            )
                .into_response(),
        );
    }
    None
}

/// If the session is already `Finished(Outcome::Submitted, Some(id))` with
/// `id` matching `mutation_id`, this is a lost-response retry of the submit
/// that already won: answers 200 immediately. Checked (and dropped) under
/// the same `phase` lock every other phase read uses, with no `await` held.
///
/// This must run BEFORE `check_head_freshness`/the verdict-completeness and
/// draft-validation checks in `post_submit`: those inspect *live* state
/// (the repository's current `HEAD`) that may have legitimately moved on in
/// the time between the original submit winning and this retry arriving —
/// exactly what freshness exists to catch for a genuinely NEW submit, but
/// it must not be able to turn a successful retry of the one that already
/// won into a stale-HEAD 409 or a validation 422. An `Aborted`/`Timeout`
/// finish, or a `Submitted` finish under a DIFFERENT id, does not match
/// here and falls through to the normal (still-409ing) path.
fn already_submitted_replay(state: &SessionState, mutation_id: &str) -> Option<Response> {
    let phase = crate::session::lock_ignore_poison(&state.phase);
    let is_replay = matches!(
        &*phase,
        crate::session::Phase::Finished(Outcome::Submitted(_), Some(id)) if id.as_str() == mutation_id
    );
    drop(phase);
    is_replay.then(|| Json(json!({"ok": true})).into_response())
}

async fn put_draft(
    State(state): State<Arc<SessionState>>,
    Path(token): Path<String>,
    Json(body): Json<DraftPut>,
) -> Response {
    if token != state.token {
        return not_found();
    }
    if let Some(invalid) = validate_mutation_id(&body.mutation_id) {
        return invalid;
    }
    // Draft and terminal state live behind the same lock, so "finished?"
    // and "write the draft" are one atomic step: a submit/abort landing
    // concurrently either happens before this lock (we see Finished and
    // refuse) or after it (it sees our completed write). A finished
    // session's draft is frozen — a late autosave must not rewrite it.
    let mut phase = crate::session::lock_ignore_poison(&state.phase);
    match &mut *phase {
        crate::session::Phase::Finished(outcome, _) => {
            let kind = crate::session::outcome_kind(outcome);
            (
                StatusCode::CONFLICT,
                Json(json!({
                    "error": "session finished",
                    "finished": kind,
                    "details": [format!("this review already ended ({kind}); nothing further can be saved")],
                })),
            )
                .into_response()
        }
        crate::session::Phase::Reviewing(slot) => {
            // Replay: a repeat of the same mutation id is treated as
            // already applied, whatever the incoming `revision` says (it
            // may be stale — the client that never saw the first response
            // has no way to know the revision it produced). This is the
            // lost-response-retry case; it does not re-apply the draft or
            // bump the revision again.
            if let Some((last_id, last_revision)) = &slot.last_save {
                if *last_id == body.mutation_id {
                    return Json(json!({"revision": *last_revision})).into_response();
                }
            }
            if body.revision != slot.revision {
                return (
                    StatusCode::CONFLICT,
                    Json(json!({
                        "error": "draft conflict",
                        "current_revision": slot.revision,
                        "details": ["the draft was changed elsewhere (another tab?); reload the review before editing further"],
                    })),
                )
                    .into_response();
            }
            slot.draft = body.draft;
            slot.revision += 1;
            slot.last_save = Some((body.mutation_id.clone(), slot.revision));
            Json(json!({"revision": slot.revision})).into_response()
        }
    }
}

/// Re-resolves `HEAD` and refuses the submit if it no longer matches the
/// commit this session was started on. Returns `None` when the submit may
/// proceed.
///
/// Without this, a human could approve the diff of commit A while the agent
/// advances `HEAD` to commit B, and the result would be misread as an
/// approval of B. Sessions without a repo behind them (demo) skip the check.
/// A git failure here fails closed (503): an unverifiable submit is refused
/// rather than emitted, but the session is not consumed, so the submit can
/// be retried.
async fn check_head_freshness(state: &SessionState) -> Option<Response> {
    let (root, expected) = match (&state.repo_root, &state.snapshot.head_oid) {
        (Some(root), Some(expected)) => (root.clone(), expected.clone()),
        _ => return None,
    };
    let resolved = tokio::task::spawn_blocking(move || {
        crate::gitdiff::rev_parse_commit_with_deadline(&root, "HEAD", HEAD_FRESHNESS_TIMEOUT)
    })
    .await;
    let current = match resolved {
        Ok(Ok(oid)) => oid,
        Ok(Err(_)) | Err(_) => {
            return Some(
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({
                        "error": "could not verify HEAD",
                        "details": ["failed to re-resolve HEAD to confirm the reviewed commit is still checked out; retry the submit"],
                    })),
                )
                    .into_response(),
            );
        }
    };
    if current != expected {
        let short = |oid: &str| oid.chars().take(12).collect::<String>();
        return Some(
            (
                StatusCode::CONFLICT,
                Json(json!({
                    "error": "review stale",
                    "details": [format!(
                        "HEAD changed since this review started ({} -> {}); the diff on screen no longer matches the repository. Close this session and start a new review.",
                        short(&expected), short(&current)
                    )],
                    "expected_head_oid": expected,
                    "current_head_oid": current,
                })),
            )
                .into_response(),
        );
    }
    None
}

async fn post_submit(
    State(state): State<Arc<SessionState>>,
    Path(token): Path<String>,
    Json(body): Json<DraftPut>,
) -> Response {
    if token != state.token {
        return not_found();
    }
    if let Some(invalid) = validate_mutation_id(&body.mutation_id) {
        return invalid;
    }
    // Lost-response retry short-circuit: must run BEFORE freshness/
    // completeness/validation below, all of which read live state that may
    // have moved on since the original submit already won. See
    // `already_submitted_replay`'s doc comment for why. `try_finish_at_revision`'s
    // `AlreadySubmittedSame` further down is the second line of defense for
    // the race where the winning submit finishes between this check and the
    // claim.
    if let Some(replay) = already_submitted_replay(&state, &body.mutation_id) {
        return replay;
    }
    let draft = body.draft;

    // Freshness first: if the reviewed commit is gone, no draft state makes
    // the submit valid, so this is the dominant error.
    if let Some(stale) = check_head_freshness(&state).await {
        return stale;
    }

    let missing: Vec<String> = state
        .required_ids()
        .into_iter()
        .filter(|id| draft.concerns.get(id).is_none_or(|cd| cd.verdict.is_none()))
        .collect();
    if !missing.is_empty() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error": "missing verdicts", "missing": missing})),
        )
            .into_response();
    }

    let details = state.validate_draft(&draft);
    if !details.is_empty() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error": "invalid draft", "details": details})),
        )
            .into_response();
    }

    // Build the result first, then claim the terminal state at the caller's
    // draft revision: revision check, terminal claim, and outcome publish
    // are one atomic step. A stale tab (another tab saved a newer draft)
    // gets the same draft-conflict refusal a stale save does — submitting
    // must not be a side door around it. Claimed only after full
    // validation, so a rejected submit never consumes the session.
    let result = state.build_result(&draft);
    match state.try_finish_at_revision(
        Outcome::Submitted(Box::new(result)),
        body.revision,
        &body.mutation_id,
    ) {
        crate::session::FinishAttempt::Won | crate::session::FinishAttempt::AlreadySubmittedSame => {
            Json(json!({"ok": true})).into_response()
        }
        crate::session::FinishAttempt::AlreadyFinished(kind) => (
            StatusCode::CONFLICT,
            Json(json!({
                "error": "session finished",
                "finished": kind,
                "details": [format!("this review already ended ({kind})")],
            })),
        )
            .into_response(),
        crate::session::FinishAttempt::RevisionConflict(current) => (
            StatusCode::CONFLICT,
            Json(json!({
                "error": "draft conflict",
                "current_revision": current,
                "details": ["the draft was changed elsewhere (another tab?); reload the review before submitting"],
            })),
        )
            .into_response(),
    }
}

/// 409 for an action against a session that already ended, naming how it
/// ended — an aborted session must not be reported as "submitted".
fn finished_conflict(state: &SessionState) -> Response {
    let kind = state.finished_kind().unwrap_or("submitted");
    (
        StatusCode::CONFLICT,
        Json(json!({
            "error": "session finished",
            "finished": kind,
            "details": [format!("this review already ended ({kind})")],
        })),
    )
        .into_response()
}

async fn post_abort(State(state): State<Arc<SessionState>>, Path(token): Path<String>) -> Response {
    if token != state.token {
        return not_found();
    }

    if !state.try_finish(Outcome::Aborted) {
        return finished_conflict(&state);
    }
    Json(json!({"ok": true})).into_response()
}

pub fn build_router(state: Arc<SessionState>) -> Router {
    let router = Router::new()
        .route("/r/{token}", get(get_index))
        .route("/assets/{*path}", get(get_asset))
        .route("/api/{token}/session", get(get_session))
        .route("/api/{token}/draft", put(put_draft))
        .route("/api/{token}/submit", post(post_submit))
        .route("/api/{token}/abort", post(post_abort))
        .with_state(state)
        .layer(axum::extract::DefaultBodyLimit::max(MAX_BODY_BYTES));
    with_middleware(router, REQUEST_TIMEOUT)
}

/// Applies the shared middleware stack. Layer order matters: the LAST
/// `.layer()` is the outermost, and `security_headers` must be outermost so
/// that even a synthesized 408 from the timeout layer (which drops the inner
/// future, headers and all) still passes through it and gets
/// no-store/CSP/etc. — every response, no exceptions, per the header
/// invariant. Split out of `build_router` so tests can exercise this exact
/// ordering with a short timeout.
fn with_middleware(router: Router, timeout: std::time::Duration) -> Router {
    router
        .layer(middleware::from_fn_with_state(timeout, request_timeout))
        .layer(middleware::from_fn(json_payload_too_large))
        .layer(middleware::from_fn(security_headers))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gitdiff::{parse_unified_diff, ChangeKind, ContentKind, FileDiff};
    use crate::mapping::resolve_mapping;
    use crate::model::{Concern, ConcernsInput, Decision, Location, Risk};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    const MODIFIED: &str = "\
diff --git a/src/app.ts b/src/app.ts
index 1111111..2222222 100644
--- a/src/app.ts
+++ b/src/app.ts
@@ -1,4 +1,5 @@ header-one
 line1
-old2
+new2
+new3
 line4
@@ -10,3 +11,3 @@ header-two
 a
-b
+B
 c
";

    const TOKEN: &str = "sesstoken";

    fn build_state() -> Arc<SessionState> {
        build_state_with_deadline(None)
    }

    /// Same session shape as `build_state`, with `deadline_at` overridable —
    /// used to exercise the `GET /session` payload's `deadline_at` field
    /// (Task 2.4) without duplicating the whole fixture for that one case.
    fn build_state_with_deadline(
        deadline_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Arc<SessionState> {
        let files = parse_unified_diff(MODIFIED);
        let input = ConcernsInput {
            version: 1,
            summary: Some("summary text".to_string()),
            concerns: vec![
                Concern {
                    id: "c1".to_string(),
                    title: "Concern one".to_string(),
                    description: Some("desc one".to_string()),
                    risk: Risk::High,
                    locations: vec![Location {
                        path: "src/app.ts".to_string(),
                        side: None,
                        start: Some(1),
                        end: Some(4),
                    }],
                },
                Concern {
                    id: "c2".to_string(),
                    title: "Concern two".to_string(),
                    description: None,
                    risk: Risk::Low,
                    locations: vec![],
                },
            ],
        };
        let mapping = resolve_mapping(&files, &input);
        // c1 claims hunk 0; c2 claims nothing; hunk 1 is left unmapped.
        assert_eq!(mapping.unmapped.len(), 1);

        let snapshot = crate::snapshot::ReviewSnapshot::without_git("main", &files, &input);
        let (tx, _rx) = tokio::sync::watch::channel(());
        Arc::new(SessionState {
            title: "review title".to_string(),
            summary: Some("summary text".to_string()),
            files,
            mapping,
            input,
            token: TOKEN.to_string(),
            session_id: "sessid".to_string(),
            snapshot,
            repo_root: None,
            started_at: chrono::Utc::now(),
            deadline_at,
            phase: std::sync::Mutex::new(crate::session::Phase::Reviewing(
                crate::session::DraftSlot::default(),
            )),
            outcome_tx: tx,
        })
    }

    /// Same session shape as `build_state`, but pinned to a real repo at
    /// `head_oid` — the only way to exercise `check_head_freshness`'s live
    /// `HEAD` re-check (it's a no-op whenever `repo_root` is `None`, which
    /// every other test in this module uses).
    fn build_state_with_repo(repo_root: std::path::PathBuf, head_oid: String) -> Arc<SessionState> {
        let files = parse_unified_diff(MODIFIED);
        let input = ConcernsInput {
            version: 1,
            summary: Some("summary text".to_string()),
            concerns: vec![
                Concern {
                    id: "c1".to_string(),
                    title: "Concern one".to_string(),
                    description: Some("desc one".to_string()),
                    risk: Risk::High,
                    locations: vec![Location {
                        path: "src/app.ts".to_string(),
                        side: None,
                        start: Some(1),
                        end: Some(4),
                    }],
                },
                Concern {
                    id: "c2".to_string(),
                    title: "Concern two".to_string(),
                    description: None,
                    risk: Risk::Low,
                    locations: vec![],
                },
            ],
        };
        let mapping = resolve_mapping(&files, &input);
        assert_eq!(mapping.unmapped.len(), 1);

        let mut snapshot = crate::snapshot::ReviewSnapshot::without_git("main", &files, &input);
        snapshot.head_oid = Some(head_oid);
        let (tx, _rx) = tokio::sync::watch::channel(());
        Arc::new(SessionState {
            title: "review title".to_string(),
            summary: Some("summary text".to_string()),
            files,
            mapping,
            input,
            token: TOKEN.to_string(),
            session_id: "sessid".to_string(),
            snapshot,
            repo_root: Some(repo_root),
            started_at: chrono::Utc::now(),
            deadline_at: None,
            phase: std::sync::Mutex::new(crate::session::Phase::Reviewing(
                crate::session::DraftSlot::default(),
            )),
            outcome_tx: tx,
        })
    }

    /// A rename+modify diff, so old-side anchors live on `old-name.ts` and
    /// new-side anchors on `new-name.ts`.
    const RENAMED: &str = "\
diff --git a/old-name.ts b/new-name.ts
similarity index 90%
rename from old-name.ts
rename to new-name.ts
index 1111111..2222222 100644
--- a/old-name.ts
+++ b/new-name.ts
@@ -1,2 +1,2 @@
 keep
-alpha
+beta
";

    fn build_rename_state() -> Arc<SessionState> {
        let files = parse_unified_diff(RENAMED);
        let input = ConcernsInput {
            version: 1,
            summary: None,
            concerns: vec![Concern {
                id: "r1".to_string(),
                title: "Rename".to_string(),
                description: None,
                risk: Risk::Low,
                locations: vec![Location {
                    path: "new-name.ts".to_string(),
                    side: None,
                    start: None,
                    end: None,
                }],
            }],
        };
        let mapping = resolve_mapping(&files, &input);
        assert!(mapping.unmapped.is_empty());

        let snapshot = crate::snapshot::ReviewSnapshot::without_git("main", &files, &input);
        let (tx, _rx) = tokio::sync::watch::channel(());
        Arc::new(SessionState {
            title: "rename review".to_string(),
            summary: None,
            files,
            mapping,
            input,
            token: TOKEN.to_string(),
            session_id: "sessid".to_string(),
            snapshot,
            repo_root: None,
            started_at: chrono::Utc::now(),
            deadline_at: None,
            phase: std::sync::Mutex::new(crate::session::Phase::Reviewing(
                crate::session::DraftSlot::default(),
            )),
            outcome_tx: tx,
        })
    }

    /// Binary ファイルを1つ含む state（c1 が whole-file location で claim）。
    fn build_opaque_state() -> Arc<SessionState> {
        let mut files = parse_unified_diff(MODIFIED);
        files.push(FileDiff {
            old_path: Some("logo.png".to_string()),
            new_path: Some("logo.png".to_string()),
            change_kind: ChangeKind::Modified,
            content_kind: ContentKind::Binary,
            old_mode: Some("100644".to_string()),
            new_mode: Some("100644".to_string()),
            old_type: Some(crate::gitdiff::FileType::Regular),
            new_type: Some(crate::gitdiff::FileType::Regular),
            old_oid: Some("1111111111111111111111111111111111111111".to_string()),
            new_oid: Some("2222222222222222222222222222222222222222".to_string()),
            old_size: Some(10),
            new_size: Some(20),
            lfs_pointer: false,
            hunks: Vec::new(),
        });
        let input = ConcernsInput {
            version: 1,
            summary: None,
            concerns: vec![Concern {
                id: "c1".to_string(),
                title: "All".to_string(),
                description: None,
                risk: Risk::Medium,
                locations: vec![
                    Location {
                        path: "src/app.ts".to_string(),
                        side: None,
                        start: None,
                        end: None,
                    },
                    Location {
                        path: "logo.png".to_string(),
                        side: None,
                        start: None,
                        end: None,
                    },
                ],
            }],
        };
        let mapping = resolve_mapping(&files, &input);
        assert!(mapping.unmapped.is_empty());

        let snapshot = crate::snapshot::ReviewSnapshot::without_git("main", &files, &input);
        let (tx, _rx) = tokio::sync::watch::channel(());
        Arc::new(SessionState {
            title: "opaque review".to_string(),
            summary: None,
            files,
            mapping,
            input,
            token: TOKEN.to_string(),
            session_id: "sessid".to_string(),
            snapshot,
            repo_root: None,
            started_at: chrono::Utc::now(),
            deadline_at: None,
            phase: std::sync::Mutex::new(crate::session::Phase::Reviewing(
                crate::session::DraftSlot::default(),
            )),
            outcome_tx: tx,
        })
    }

    async fn call(app: Router, req: http::Request<Body>) -> (StatusCode, serde_json::Value) {
        let res = app.oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let json = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        };
        (status, json)
    }

    fn get(path: &str) -> http::Request<Body> {
        http::Request::builder()
            .method("GET")
            .uri(path)
            .body(Body::empty())
            .unwrap()
    }

    fn put_json(path: &str, body: serde_json::Value) -> http::Request<Body> {
        http::Request::builder()
            .method("PUT")
            .uri(path)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    fn post_json(path: &str, body: serde_json::Value) -> http::Request<Body> {
        http::Request::builder()
            .method("POST")
            .uri(path)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    fn post_empty(path: &str) -> http::Request<Body> {
        http::Request::builder()
            .method("POST")
            .uri(path)
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn get_session_ok() {
        let state = build_state();
        let app = build_router(state.clone());
        let (status, body) = call(app, get(&format!("/api/{TOKEN}/session"))).await;
        assert_eq!(status, StatusCode::OK);
        let concerns = body["concerns"].as_array().unwrap();
        assert_eq!(concerns.len(), 3);
        assert_eq!(concerns[0]["id"], "c1");
        assert_eq!(concerns[1]["id"], "c2");
        assert_eq!(concerns[2]["id"], "_unmapped");
        assert_eq!(concerns[2]["unmapped"], true);
        assert_eq!(body["finished"], serde_json::Value::Null);
        // No --timeout given: no deadline to report.
        assert_eq!(body["deadline_at"], serde_json::Value::Null);
    }

    /// Task 2.4: when the session was started with `--timeout`, the payload
    /// carries the RFC3339 deadline so the UI can render a countdown.
    #[tokio::test]
    async fn get_session_reports_deadline_at_when_timeout_set() {
        let deadline = chrono::Utc::now() + chrono::Duration::minutes(30);
        let state = build_state_with_deadline(Some(deadline));
        let app = build_router(state.clone());
        let (status, body) = call(app, get(&format!("/api/{TOKEN}/session"))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["deadline_at"], deadline.to_rfc3339());
    }

    #[tokio::test]
    async fn wrong_token_404() {
        let state = build_state();
        let app = build_router(state.clone());
        let (status, body) = call(app, get("/api/deadbeef/session")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body.is_null());
    }

    #[tokio::test]
    async fn draft_roundtrip() {
        let state = build_state();
        let app = build_router(state.clone());

        let draft_body = json!({
            "revision": 0,
            "mutation_id": "m1",
            "draft": {
                "concerns": { "c1": { "verdict": "approve", "comments": [] } },
                "general_comments": []
            }
        });
        let (status, body) = call(
            app.clone(),
            put_json(&format!("/api/{TOKEN}/draft"), draft_body),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body: {body}");
        assert_eq!(body["revision"], 1);

        let (status, body) = call(app, get(&format!("/api/{TOKEN}/session"))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["draft"]["concerns"]["c1"]["verdict"], "approve");
        assert_eq!(body["draft_revision"], 1);
        assert_eq!(body["limits"]["max_comment_chars"], 10_000);
        assert_eq!(body["limits"]["max_comments"], 500);
    }

    /// A save presenting a stale revision must be refused (409) without
    /// overwriting the newer draft — this is what stops two tabs from
    /// silently clobbering each other via last-write-wins.
    #[tokio::test]
    async fn stale_draft_revision_conflicts_without_overwrite() {
        let state = build_state();
        let app = build_router(state.clone());

        let save = |verdict: &str, revision: u64, mutation_id: &str| {
            json!({
                "revision": revision,
                "mutation_id": mutation_id,
                "draft": {
                    "concerns": { "c1": { "verdict": verdict, "comments": [] } },
                    "general_comments": []
                }
            })
        };
        let (status, _) = call(
            app.clone(),
            put_json(&format!("/api/{TOKEN}/draft"), save("approve", 0, "tab-a")),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        // A second writer still holding revision 0 (the other tab) — a
        // genuinely different mutation id, not a retry of the first save.
        let (status, body) = call(
            app.clone(),
            put_json(
                &format!("/api/{TOKEN}/draft"),
                save("request-changes", 0, "tab-b"),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "body: {body}");
        assert_eq!(body["error"], "draft conflict");
        assert_eq!(body["current_revision"], 1);

        // The first tab's save survived.
        let (_, body) = call(app, get(&format!("/api/{TOKEN}/session"))).await;
        assert_eq!(body["draft"]["concerns"]["c1"]["verdict"], "approve");
    }

    /// Submitting with a stale revision must be refused exactly like a
    /// stale save — submit is not a side door around the conflict
    /// protection — and must not consume the session: a submit at the
    /// current revision afterwards succeeds.
    #[tokio::test]
    async fn stale_revision_submit_conflicts_without_consuming_session() {
        let state = build_state();
        let app = build_router(state.clone());

        // Another tab saved once: revision is now 1.
        let save = json!({
            "revision": 0,
            "mutation_id": "other-tab-save",
            "draft": { "concerns": {}, "general_comments": [] }
        });
        let (status, _) = call(app.clone(), put_json(&format!("/api/{TOKEN}/draft"), save)).await;
        assert_eq!(status, StatusCode::OK);

        // The stale tab (still at revision 0) tries to submit.
        let (status, body) = call(
            app.clone(),
            post_json(
                &format!("/api/{TOKEN}/submit"),
                submit_body(complete_draft()),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "body: {body}");
        assert_eq!(body["error"], "draft conflict");
        assert_eq!(body["current_revision"], 1);
        assert!(state.finished_outcome().is_none(), "session must survive");

        // The up-to-date tab submits at the current revision.
        let (status, body) = call(
            app,
            post_json(
                &format!("/api/{TOKEN}/submit"),
                json!({
                    "revision": 1,
                    "mutation_id": "up-to-date-tab-submit",
                    "draft": complete_draft()
                }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body: {body}");
        assert!(matches!(
            state.finished_outcome().unwrap(),
            Outcome::Submitted(_)
        ));
    }

    /// After the session finishes, the draft is frozen: a late autosave gets
    /// 409 instead of rewriting what the outcome was built from.
    #[tokio::test]
    async fn draft_save_after_finish_conflicts() {
        let state = build_state();
        let app = build_router(state.clone());
        let (status, _) = call(
            app.clone(),
            post_json(
                &format!("/api/{TOKEN}/submit"),
                submit_body(complete_draft()),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let draft_body = json!({
            "revision": 0,
            "mutation_id": "late-autosave",
            "draft": { "concerns": {}, "general_comments": [] }
        });
        let (status, body) = call(app, put_json(&format!("/api/{TOKEN}/draft"), draft_body)).await;
        assert_eq!(status, StatusCode::CONFLICT, "body: {body}");
        assert_eq!(body["error"], "session finished");
        assert_eq!(body["finished"], "submitted");
    }

    /// A save landing after an abort must say the session was aborted, not
    /// "submitted" — the tab autosaving during an abort would otherwise
    /// switch to the submitted screen.
    #[tokio::test]
    async fn draft_save_after_abort_names_the_abort() {
        let state = build_state();
        let app = build_router(state.clone());
        let (status, _) = call(app.clone(), post_empty(&format!("/api/{TOKEN}/abort"))).await;
        assert_eq!(status, StatusCode::OK);

        let draft_body = json!({
            "revision": 0,
            "mutation_id": "late-autosave",
            "draft": { "concerns": {}, "general_comments": [] }
        });
        let (status, body) = call(
            app.clone(),
            put_json(&format!("/api/{TOKEN}/draft"), draft_body),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "body: {body}");
        assert_eq!(body["error"], "session finished");
        assert_eq!(body["finished"], "aborted");

        // The session payload names the ending too.
        let (_, body) = call(app, get(&format!("/api/{TOKEN}/session"))).await;
        assert_eq!(body["finished"], "aborted");
    }

    /// An oversized body gets the same JSON error shape as every other
    /// refusal, not a bare 413.
    #[tokio::test]
    async fn oversized_draft_body_gets_json_413() {
        let state = build_state();
        let app = build_router(state.clone());
        let huge = "x".repeat(MAX_BODY_BYTES + 1024);
        let draft_body = json!({
            "revision": 0,
            "mutation_id": "oversized",
            "draft": { "concerns": {}, "general_comments": [huge] }
        });
        let (status, body) = call(app, put_json(&format!("/api/{TOKEN}/draft"), draft_body)).await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(body["error"], "payload too large");
    }

    #[tokio::test]
    async fn draft_roundtrip_acknowledged_opaque() {
        let state = build_opaque_state();
        let app = build_router(state.clone());

        let draft_body = json!({
            "revision": 0,
            "mutation_id": "ack-opaque",
            "draft": {
                "concerns": {},
                "general_comments": [],
                "acknowledged_opaque": [1]
            }
        });
        let (status, body) = call(
            app.clone(),
            put_json(&format!("/api/{TOKEN}/draft"), draft_body),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body: {body}");

        let (status, body) = call(app, get(&format!("/api/{TOKEN}/session"))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["draft"]["acknowledged_opaque"], json!([1]));
    }

    /// A save whose response was lost (client timed out, but the server
    /// already applied it) must not corrupt state on retry: resending the
    /// exact same `mutation_id` replays the recorded post-apply revision
    /// instead of re-applying — even though the retry still carries the OLD
    /// `revision` (the client never learned the new one), and even though
    /// the retry's draft content differs (a real client would resend the
    /// identical draft; this exercises that the replay path really does
    /// ignore the incoming draft rather than merely happening to match).
    #[tokio::test]
    async fn save_retry_with_same_mutation_id_is_idempotent() {
        let state = build_state();
        let app = build_router(state.clone());

        let first = json!({
            "revision": 0,
            "mutation_id": "save-1",
            "draft": {
                "concerns": { "c1": { "verdict": "approve", "comments": [] } },
                "general_comments": []
            }
        });
        let (status, body) =
            call(app.clone(), put_json(&format!("/api/{TOKEN}/draft"), first)).await;
        assert_eq!(status, StatusCode::OK, "body: {body}");
        assert_eq!(body["revision"], 1);

        let retry = json!({
            "revision": 0,
            "mutation_id": "save-1",
            "draft": {
                "concerns": { "c1": { "verdict": "request-changes", "comments": [] } },
                "general_comments": []
            }
        });
        let (status, body) =
            call(app.clone(), put_json(&format!("/api/{TOKEN}/draft"), retry)).await;
        assert_eq!(status, StatusCode::OK, "body: {body}");
        assert_eq!(
            body["revision"], 1,
            "replay must answer with the recorded post-apply revision"
        );

        // The retry's differing draft must not have been applied — the
        // original save still stands, and the revision did not bump again.
        let (_, body) = call(app, get(&format!("/api/{TOKEN}/session"))).await;
        assert_eq!(body["draft"]["concerns"]["c1"]["verdict"], "approve");
        assert_eq!(body["draft_revision"], 1);
    }

    /// A different mutation id presenting a stale revision is a genuinely
    /// new (not retried) mutation, so it must still 409 exactly like
    /// `stale_draft_revision_conflicts_without_overwrite` — idempotency
    /// must not become a side door around the conflict check.
    #[tokio::test]
    async fn save_different_id_stale_revision_still_409() {
        let state = build_state();
        let app = build_router(state.clone());

        let first = json!({
            "revision": 0,
            "mutation_id": "save-1",
            "draft": { "concerns": {}, "general_comments": [] }
        });
        let (status, _) = call(app.clone(), put_json(&format!("/api/{TOKEN}/draft"), first)).await;
        assert_eq!(status, StatusCode::OK);

        let second = json!({
            "revision": 0,
            "mutation_id": "save-2",
            "draft": { "concerns": {}, "general_comments": ["different"] }
        });
        let (status, body) = call(app, put_json(&format!("/api/{TOKEN}/draft"), second)).await;
        assert_eq!(status, StatusCode::CONFLICT, "body: {body}");
        assert_eq!(body["error"], "draft conflict");
        assert_eq!(body["current_revision"], 1);
    }

    /// An empty or oversized `mutation_id` is refused with a 422 instead of
    /// panicking or being silently accepted.
    #[tokio::test]
    async fn save_invalid_mutation_id_422() {
        let state = build_state();
        let app = build_router(state.clone());

        let empty = json!({
            "revision": 0,
            "mutation_id": "",
            "draft": { "concerns": {}, "general_comments": [] }
        });
        let (status, body) =
            call(app.clone(), put_json(&format!("/api/{TOKEN}/draft"), empty)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {body}");
        assert_eq!(body["error"], "invalid mutation id");

        let oversized = json!({
            "revision": 0,
            "mutation_id": "x".repeat(101),
            "draft": { "concerns": {}, "general_comments": [] }
        });
        let (status, body) = call(app, put_json(&format!("/api/{TOKEN}/draft"), oversized)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {body}");
        assert_eq!(body["error"], "invalid mutation id");
    }

    /// Same as `save_invalid_mutation_id_422`, for `POST /submit`: an empty
    /// or oversized `mutation_id` is refused with 422, not a panic.
    #[tokio::test]
    async fn submit_invalid_mutation_id_422() {
        let state = build_state();
        let app = build_router(state.clone());

        let empty = json!({"revision": 0, "mutation_id": "", "draft": complete_draft()});
        let (status, body) = call(
            app.clone(),
            post_json(&format!("/api/{TOKEN}/submit"), empty),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {body}");
        assert_eq!(body["error"], "invalid mutation id");

        let oversized = json!({
            "revision": 0,
            "mutation_id": "x".repeat(101),
            "draft": complete_draft()
        });
        let (status, body) = call(app, post_json(&format!("/api/{TOKEN}/submit"), oversized)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {body}");
        assert_eq!(body["error"], "invalid mutation id");
    }

    #[tokio::test]
    async fn submit_incomplete_422() {
        let state = build_state();
        let app = build_router(state.clone());

        let draft_body = json!({
            "concerns": { "c1": { "verdict": "approve", "comments": [] } },
            "general_comments": []
        });
        let (status, body) = call(
            app,
            post_json(&format!("/api/{TOKEN}/submit"), submit_body(draft_body)),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        let missing: Vec<String> = body["missing"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert!(missing.contains(&"c2".to_string()));
        assert!(missing.contains(&"_unmapped".to_string()));
    }

    #[tokio::test]
    async fn submit_complete_emits_outcome() {
        let state = build_state();
        let app = build_router(state.clone());

        let draft_body = json!({
            "concerns": {
                "c1": { "verdict": "request-changes", "comments": [] },
                "c2": { "verdict": "approve", "comments": [] },
                "_unmapped": { "verdict": "approve", "comments": [] }
            },
            "general_comments": ["looks mostly fine"]
        });
        let (status, body) = call(
            app,
            post_json(&format!("/api/{TOKEN}/submit"), submit_body(draft_body)),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], true);

        let outcome = state.finished_outcome().unwrap();
        match outcome {
            Outcome::Submitted(r) => {
                assert_eq!(r.decision, Decision::RequestChanges);
                let ids: Vec<&str> = r.concerns.iter().map(|c| c.id.as_str()).collect();
                assert_eq!(ids, vec!["c1", "c2", "_unmapped"]);
            }
            other => panic!("expected Submitted, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn submit_request_changes_without_reason_422() {
        let state = build_state();
        let app = build_router(state.clone());
        let draft = json!({
            "concerns": {
                "c1": { "verdict": "request-changes", "comments": [] },
                "c2": { "verdict": "approve", "comments": [] },
                "_unmapped": { "verdict": "approve", "comments": [] }
            },
            "general_comments": []
        });
        let (status, body) = call(
            app,
            post_json(&format!("/api/{TOKEN}/submit"), submit_body(draft)),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {body}");
        assert!(body["details"]
            .to_string()
            .contains("request-changes requires a comment"));
    }

    #[tokio::test]
    async fn submit_request_changes_with_general_comment_succeeds() {
        let state = build_state();
        let app = build_router(state.clone());
        let draft = json!({
            "concerns": {
                "c1": { "verdict": "request-changes", "comments": [] },
                "c2": { "verdict": "approve", "comments": [] },
                "_unmapped": { "verdict": "approve", "comments": [] }
            },
            "general_comments": ["fix the auth check"]
        });
        let (status, body) = call(
            app,
            post_json(&format!("/api/{TOKEN}/submit"), submit_body(draft)),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body: {body}");
        assert!(matches!(
            state.finished_outcome().unwrap(),
            Outcome::Submitted(_)
        ));
    }

    #[tokio::test]
    async fn second_submit_409() {
        let state = build_state();
        let app = build_router(state.clone());

        let draft_body = json!({
            "concerns": {
                "c1": { "verdict": "approve", "comments": [] },
                "c2": { "verdict": "approve", "comments": [] },
                "_unmapped": { "verdict": "approve", "comments": [] }
            },
            "general_comments": []
        });
        let (status, _) = call(
            app.clone(),
            post_json(
                &format!("/api/{TOKEN}/submit"),
                submit_body(draft_body.clone()),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        state.finished_outcome().unwrap();

        // A genuinely different submit attempt (different mutation id),
        // not a retry of the one that just won — must still 409.
        let (status, body) = call(
            app,
            post_json(
                &format!("/api/{TOKEN}/submit"),
                json!({"revision": 0, "mutation_id": "second-attempt", "draft": draft_body}),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"], "session finished");
        assert_eq!(body["finished"], "submitted");
    }

    /// A repeat submit with the SAME `mutation_id` as the one that already
    /// won is a lost-response retry, not a new attempt: it must answer 200
    /// (not the usual "session finished" 409), and must not otherwise
    /// disturb the already-published outcome.
    #[tokio::test]
    async fn submit_retry_with_same_mutation_id_returns_submitted() {
        let state = build_state();
        let app = build_router(state.clone());

        let draft_body = complete_draft();
        let body_with_id =
            |id: &str| json!({"revision": 0, "mutation_id": id, "draft": draft_body.clone()});

        let (status, _) = call(
            app.clone(),
            post_json(&format!("/api/{TOKEN}/submit"), body_with_id("submit-1")),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let first_outcome = state.finished_outcome().unwrap();

        let (status, body) = call(
            app,
            post_json(&format!("/api/{TOKEN}/submit"), body_with_id("submit-1")),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body: {body}");
        assert_eq!(body["ok"], true);
        assert!(matches!(
            state.finished_outcome().unwrap(),
            Outcome::Submitted(_)
        ));
        // The outcome recorded by the winning submit is untouched by the
        // replay — same session id, same submitted_at timestamp.
        match (first_outcome, state.finished_outcome().unwrap()) {
            (Outcome::Submitted(a), Outcome::Submitted(b)) => {
                assert_eq!(a.submitted_at, b.submitted_at);
            }
            other => panic!("expected two Submitted outcomes: {other:?}"),
        }
    }

    /// A DIFFERENT mutation id submitted after the session already finished
    /// must still 409 exactly as before — idempotent replay must not become
    /// a side door that lets any submit through once one has won.
    #[tokio::test]
    async fn submit_different_id_after_finish_409() {
        let state = build_state();
        let app = build_router(state.clone());

        let draft_body = complete_draft();
        let (status, _) = call(
            app.clone(),
            post_json(
                &format!("/api/{TOKEN}/submit"),
                json!({"revision": 0, "mutation_id": "submit-1", "draft": draft_body.clone()}),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, body) = call(
            app,
            post_json(
                &format!("/api/{TOKEN}/submit"),
                json!({"revision": 0, "mutation_id": "submit-2", "draft": draft_body}),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "body: {body}");
        assert_eq!(body["error"], "session finished");
        assert_eq!(body["finished"], "submitted");
    }

    /// A submit's HTTP response can be lost (client-side timeout/network
    /// failure) even though the server already committed to `Submitted`.
    /// Retrying with the SAME mutation id must still replay 200 even if the
    /// live `HEAD` has moved on in the meantime (e.g. an agent pushed
    /// another commit while the response was in flight back to the
    /// client): freshness exists to catch a NEW submit against a diff
    /// that's no longer current, not to un-approve a retry of the one that
    /// already won. This is only observable with a real `repo_root`
    /// (`check_head_freshness` is a no-op otherwise), hence
    /// `build_state_with_repo` instead of the usual `build_state`.
    ///
    /// Before the fix (`already_submitted_replay` added before
    /// `check_head_freshness` in `post_submit`), this test failed: the
    /// retry hit the live `HEAD` re-check first and got 409 "review stale"
    /// instead of replaying 200.
    #[tokio::test]
    async fn submit_same_id_replays_200_even_after_head_moved() {
        let td = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let st = std::process::Command::new("git")
                .current_dir(td.path())
                .args(args)
                .output()
                .unwrap();
            assert!(
                st.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&st.stderr)
            );
        };
        git(&["init", "-b", "main"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(td.path().join("a.txt"), "one\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "base"]);
        let head_oid = crate::gitdiff::rev_parse_commit(td.path(), "HEAD").unwrap();

        let state = build_state_with_repo(td.path().to_path_buf(), head_oid);
        let app = build_router(state.clone());

        let submit = json!({
            "revision": 0,
            "mutation_id": "retry-me",
            "draft": complete_draft()
        });
        let (status, body) = call(
            app.clone(),
            post_json(&format!("/api/{TOKEN}/submit"), submit.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body: {body}");

        // HEAD moves on after the winning submit — a NEW commit lands while
        // the (lost) response was supposedly in flight back to the client.
        std::fs::write(td.path().join("a.txt"), "one\nmore\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "moved after submit"]);

        // The retry, same mutation id: must replay 200, not 409/503 off the
        // now-mismatched HEAD.
        let (status, body) = call(app, post_json(&format!("/api/{TOKEN}/submit"), submit)).await;
        assert_eq!(status, StatusCode::OK, "body: {body}");
        assert_eq!(body["ok"], true);
        assert!(matches!(
            state.finished_outcome().unwrap(),
            Outcome::Submitted(_)
        ));
    }

    #[tokio::test]
    async fn abort_emits_outcome() {
        let state = build_state();
        let app = build_router(state.clone());

        let (status, body) = call(app, post_empty(&format!("/api/{TOKEN}/abort"))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], true);

        let outcome = state.finished_outcome().unwrap();
        assert!(matches!(outcome, Outcome::Aborted));
    }

    /// Draft with all three verdicts set and the given comments on `c1`.
    fn draft_with_c1_comments(comments: serde_json::Value) -> serde_json::Value {
        json!({
            "concerns": {
                "c1": { "verdict": "approve", "comments": comments },
                "c2": { "verdict": "approve", "comments": [] },
                "_unmapped": { "verdict": "approve", "comments": [] }
            },
            "general_comments": []
        })
    }

    /// Submits `draft` and asserts a 422 "invalid draft" whose details all
    /// mention every string in `expect_in_details`.
    async fn assert_invalid_draft(draft: serde_json::Value, expect_in_details: &[&str]) {
        let state = build_state();
        let app = build_router(state.clone());
        let (status, body) = call(
            app,
            post_json(&format!("/api/{TOKEN}/submit"), submit_body(draft)),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {body}");
        assert_eq!(body["error"], "invalid draft");
        let details = body["details"].as_array().unwrap();
        assert!(!details.is_empty());
        let joined = details
            .iter()
            .map(|d| d.as_str().unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        for needle in expect_in_details {
            assert!(
                joined.contains(needle),
                "details missing {needle:?}: {joined}"
            );
        }
    }

    #[tokio::test]
    async fn submit_comment_on_nonexistent_path_422() {
        let comments = json!([
            {"path": "nope.ts", "side": "new", "line": 1, "body": "x"}
        ]);
        assert_invalid_draft(draft_with_c1_comments(comments), &["c1", "nope.ts"]).await;
    }

    #[tokio::test]
    async fn submit_comment_on_nonexistent_line_422() {
        let comments = json!([
            {"path": "src/app.ts", "side": "new", "line": 999, "body": "x"}
        ]);
        assert_invalid_draft(
            draft_with_c1_comments(comments),
            &["c1", "src/app.ts", "999"],
        )
        .await;
    }

    #[tokio::test]
    async fn submit_comment_on_unassigned_hunk_line_422() {
        // New line 12 exists in the diff, but in hunk 1, which belongs to
        // `_unmapped`, not to c1.
        let comments = json!([
            {"path": "src/app.ts", "side": "new", "line": 12, "body": "x"}
        ]);
        assert_invalid_draft(
            draft_with_c1_comments(comments),
            &["c1", "src/app.ts", "12"],
        )
        .await;
    }

    #[tokio::test]
    async fn submit_blank_comment_body_422() {
        let comments = json!([
            {"path": "src/app.ts", "side": "new", "line": 1, "body": "   "}
        ]);
        assert_invalid_draft(draft_with_c1_comments(comments), &["c1", "blank"]).await;
    }

    #[tokio::test]
    async fn submit_unknown_concern_id_422() {
        let draft = json!({
            "concerns": {
                "c1": { "verdict": "approve", "comments": [] },
                "c2": { "verdict": "approve", "comments": [] },
                "_unmapped": { "verdict": "approve", "comments": [] },
                "ghost": { "verdict": "approve", "comments": [] }
            },
            "general_comments": []
        });
        assert_invalid_draft(draft, &["ghost"]).await;
    }

    #[tokio::test]
    async fn submit_blank_general_comment_422() {
        let draft = json!({
            "concerns": {
                "c1": { "verdict": "approve", "comments": [] },
                "c2": { "verdict": "approve", "comments": [] },
                "_unmapped": { "verdict": "approve", "comments": [] }
            },
            "general_comments": [" "]
        });
        assert_invalid_draft(draft, &["general comment"]).await;
    }

    #[tokio::test]
    async fn submit_valid_anchors_200() {
        let state = build_state();
        let app = build_router(state.clone());

        // Hunk 0 (c1): context line old side, removed line old side, added
        // line new side. Hunk 1 (_unmapped): added line new side.
        let draft = json!({
            "concerns": {
                "c1": { "verdict": "approve", "comments": [
                    {"path": "src/app.ts", "side": "old", "line": 1, "body": "context old side"},
                    {"path": "src/app.ts", "side": "old", "line": 2, "body": "removed line old side"},
                    {"path": "src/app.ts", "side": "new", "line": 2, "body": "added line new side"}
                ]},
                "c2": { "verdict": "approve", "comments": [] },
                "_unmapped": { "verdict": "request-changes", "comments": [
                    {"path": "src/app.ts", "side": "new", "line": 12, "body": "unmapped hunk line"}
                ]}
            },
            "general_comments": []
        });
        let (status, body) = call(
            app,
            post_json(&format!("/api/{TOKEN}/submit"), submit_body(draft)),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body: {body}");

        let outcome = state.finished_outcome().unwrap();
        match outcome {
            Outcome::Submitted(r) => {
                assert_eq!(r.version, 2);
                assert_eq!(r.review.base_ref, "main");
                assert!(r.review.base_oid.is_none(), "no git behind test sessions");
                assert_eq!(r.concerns[0].comments.len(), 3);
            }
            other => panic!("expected Submitted, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn submit_old_side_against_new_path_422_then_old_path_200() {
        let state = build_rename_state();
        let app = build_router(state.clone());

        // The old side of a renamed file anchors on old-name.ts, so the
        // same line addressed via new-name.ts on the old side must fail...
        let bad = json!({
            "concerns": { "r1": { "verdict": "approve", "comments": [
                {"path": "new-name.ts", "side": "old", "line": 2, "body": "x"}
            ]}},
            "general_comments": []
        });
        let (status, body) = call(
            app.clone(),
            post_json(&format!("/api/{TOKEN}/submit"), submit_body(bad)),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {body}");
        assert_eq!(body["error"], "invalid draft");

        // ...and a rejected submit must not consume the session: the same
        // anchor via old-name.ts succeeds afterwards.
        let good = json!({
            "concerns": { "r1": { "verdict": "approve", "comments": [
                {"path": "old-name.ts", "side": "old", "line": 2, "body": "x"}
            ]}},
            "general_comments": []
        });
        let (status, body) = call(
            app,
            post_json(&format!("/api/{TOKEN}/submit"), submit_body(good)),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body: {body}");
        assert!(matches!(
            state.finished_outcome().unwrap(),
            Outcome::Submitted(_)
        ));
    }

    #[tokio::test]
    async fn submit_without_opaque_ack_422() {
        let state = build_opaque_state();
        let app = build_router(state.clone());
        let draft = json!({
            "concerns": { "c1": { "verdict": "approve", "comments": [] } },
            "general_comments": []
        });
        let (status, body) = call(
            app,
            post_json(&format!("/api/{TOKEN}/submit"), submit_body(draft)),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {body}");
        assert!(body["details"].to_string().contains("logo.png"));
    }

    #[tokio::test]
    async fn submit_with_opaque_ack_succeeds() {
        let state = build_opaque_state();
        let app = build_router(state.clone());
        let draft = json!({
            "concerns": { "c1": { "verdict": "approve", "comments": [] } },
            "general_comments": [],
            "acknowledged_opaque": [1]
        });
        let (status, body) = call(
            app,
            post_json(&format!("/api/{TOKEN}/submit"), submit_body(draft)),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body: {body}");
        assert!(matches!(
            state.finished_outcome().unwrap(),
            Outcome::Submitted(_)
        ));
    }

    #[tokio::test]
    async fn submit_ack_on_non_opaque_file_422() {
        let state = build_opaque_state();
        let app = build_router(state.clone());
        let draft = json!({
            "concerns": { "c1": { "verdict": "approve", "comments": [] } },
            "general_comments": [],
            "acknowledged_opaque": [0, 1, 99]
        });
        let (status, body) = call(
            app,
            post_json(&format!("/api/{TOKEN}/submit"), submit_body(draft)),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "body: {body}");
    }

    /// Regression test for the embedded-asset 404 bug: the wildcard
    /// `/assets/{*path}` route was passing the captured suffix straight to
    /// `assets::serve`, but `rust-embed` keys assets under an `assets/`
    /// prefix (matching `frontend/dist/assets/...` on disk), so every real
    /// asset request 404'd. Picks a real key from the embedded set rather
    /// than hardcoding a filename, since Vite's output hash changes on every
    /// build.
    #[tokio::test]
    async fn get_asset_serves_embedded_file() {
        let key = assets::Asset::iter()
            .find(|p| p.starts_with("assets/"))
            .unwrap_or_else(|| {
                panic!(
                    "no embedded asset under 'assets/' found; expected build.rs to have run vite build"
                )
            });
        let suffix = key.strip_prefix("assets/").unwrap();

        let state = build_state();
        let app = build_router(state.clone());
        let res = app
            .oneshot(get(&format!("/assets/{suffix}")))
            .await
            .unwrap();

        assert_eq!(res.status(), StatusCode::OK);
        let content_type = res
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .expect("expected a Content-Type header");
        assert!(!content_type.is_empty());
        assert_eq!(
            res.headers()
                .get(axum::http::header::CACHE_CONTROL)
                .unwrap(),
            "public, max-age=31536000, immutable",
            "hashed asset filenames from Vite are immutable, so assets get a long-lived cache"
        );
        assert_eq!(
            res.headers()
                .get(axum::http::header::REFERRER_POLICY)
                .unwrap(),
            "no-referrer"
        );
        assert_eq!(
            res.headers()
                .get(axum::http::header::X_CONTENT_TYPE_OPTIONS)
                .unwrap(),
            "nosniff"
        );
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        assert!(!bytes.is_empty(), "expected non-empty asset body");
    }

    /// Every non-asset response (index / API) must never be cached, since the
    /// session token lives in the URL path — a shared cache or history entry
    /// leaking it would leak session access.
    #[tokio::test]
    async fn api_response_has_security_and_no_store_headers() {
        let state = build_state();
        let app = build_router(state.clone());
        let res = app
            .oneshot(get(&format!("/api/{TOKEN}/session")))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let headers = res.headers();
        assert_eq!(
            headers.get(axum::http::header::REFERRER_POLICY).unwrap(),
            "no-referrer"
        );
        assert_eq!(
            headers
                .get(axum::http::header::X_CONTENT_TYPE_OPTIONS)
                .unwrap(),
            "nosniff"
        );
        assert_eq!(
            headers.get(axum::http::header::CACHE_CONTROL).unwrap(),
            "no-store"
        );

        let csp = headers
            .get(axum::http::header::CONTENT_SECURITY_POLICY)
            .expect("expected a Content-Security-Policy header")
            .to_str()
            .unwrap();
        assert!(csp.contains("default-src 'self'"));
        assert!(csp.contains("script-src 'self'"));
        assert!(csp.contains("style-src 'self' 'unsafe-inline'"));
        assert!(csp.contains("font-src 'self' data:"));
        assert!(csp.contains("connect-src 'self'"));
        assert!(csp.contains("img-src 'self' data:"));
        assert!(csp.contains("frame-ancestors 'none'"));
        assert!(csp.contains("base-uri 'none'"));
    }

    /// A path that matches no route at all (not `/r/*`, not `/assets/*`, not
    /// `/api/*`) still goes through `security_headers`, since the middleware
    /// wraps the whole router including its fallback 404 — this must not
    /// regress into an uncached bare 404 with no security headers.
    #[tokio::test]
    async fn unmatched_path_404_has_no_store_and_csp() {
        let state = build_state();
        let app = build_router(state.clone());
        let res = app.oneshot(get("/totally/unknown/path")).await.unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);

        let headers = res.headers();
        assert_eq!(
            headers.get(axum::http::header::CACHE_CONTROL).unwrap(),
            "no-store"
        );
        assert!(
            headers
                .get(axum::http::header::CONTENT_SECURITY_POLICY)
                .is_some(),
            "expected a Content-Security-Policy header on an unmatched 404"
        );
    }

    /// Wraps a draft in the submit wire shape at revision 0 (tests that
    /// never save keep revision 0), with a fixed mutation id — fine for
    /// every test here since each only submits once (a test that submits
    /// twice with a specific idempotency intent supplies its own body
    /// instead, e.g. `second_submit_409`).
    fn submit_body(draft: serde_json::Value) -> serde_json::Value {
        json!({"revision": 0, "mutation_id": "test-submit", "draft": draft})
    }

    /// Complete draft with every required verdict approved.
    fn complete_draft() -> serde_json::Value {
        json!({
            "concerns": {
                "c1": { "verdict": "approve", "comments": [] },
                "c2": { "verdict": "approve", "comments": [] },
                "_unmapped": { "verdict": "approve", "comments": [] }
            },
            "general_comments": []
        })
    }

    /// Race a submit against an abort many times: every round must resolve
    /// to exactly one HTTP 200, one 409, and a published outcome matching
    /// the 200 winner. This is the regression net for the old
    /// terminal-claim/outcome-send gap, where a winner could claim the
    /// terminal state and then fail to deliver the outcome.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_submit_abort_resolves_to_exactly_one_outcome() {
        for round in 0..250 {
            let state = build_state();
            let app = build_router(state.clone());

            let submit_app = app.clone();
            let submit = tokio::spawn(async move {
                call(
                    submit_app,
                    post_json(
                        &format!("/api/{TOKEN}/submit"),
                        submit_body(complete_draft()),
                    ),
                )
                .await
            });
            let abort_app = app.clone();
            let abort = tokio::spawn(async move {
                call(abort_app, post_empty(&format!("/api/{TOKEN}/abort"))).await
            });

            let (submit_res, abort_res) = (submit.await.unwrap(), abort.await.unwrap());
            let winners = [submit_res.0, abort_res.0]
                .iter()
                .filter(|s| **s == StatusCode::OK)
                .count();
            assert_eq!(
                winners, 1,
                "round {round}: expected exactly one winner, got submit={} abort={}",
                submit_res.0, abort_res.0
            );

            let outcome = state
                .finished_outcome()
                .expect("a winner must have published an outcome");
            match (submit_res.0 == StatusCode::OK, &outcome) {
                (true, Outcome::Submitted(_)) | (false, Outcome::Aborted) => {}
                other => panic!("round {round}: HTTP winner and outcome disagree: {other:?}"),
            }
        }
    }

    /// A handler that overruns the request timeout gets a 408 instead of
    /// holding its connection open forever — and, because the test goes
    /// through `with_middleware` (the exact production stack/order), the
    /// synthesized 408 must still carry the security headers: the timeout
    /// layer drops the inner future, so only a `security_headers`-outermost
    /// ordering can add them.
    #[tokio::test]
    async fn overrunning_request_gets_408_with_security_headers() {
        let app = with_middleware(
            Router::new().route(
                "/slow",
                axum::routing::get(|| async {
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                    "too late"
                }),
            ),
            std::time::Duration::from_millis(20),
        );
        let res = app.oneshot(get("/slow")).await.unwrap();
        assert_eq!(res.status(), StatusCode::REQUEST_TIMEOUT);

        let headers = res.headers();
        assert_eq!(
            headers.get(axum::http::header::CACHE_CONTROL).unwrap(),
            "no-store",
            "a synthesized 408 must not bypass the security headers"
        );
        assert!(headers
            .get(axum::http::header::CONTENT_SECURITY_POLICY)
            .is_some());
        assert_eq!(
            headers.get(axum::http::header::REFERRER_POLICY).unwrap(),
            "no-referrer"
        );
        assert_eq!(
            headers
                .get(axum::http::header::X_CONTENT_TYPE_OPTIONS)
                .unwrap(),
            "nosniff"
        );

        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"], "request timed out");
    }

    /// The timeout hierarchy for a submit must be internal deadline < server
    /// request timeout < client fetch timeout: otherwise a client can give
    /// up and show "failed" while the server (and the rev-parse
    /// `check_head_freshness` spawn_blocking's onto it) is still working,
    /// which can commit the submit server-side after the browser already
    /// told the user it didn't happen. `HEAD_FRESHNESS_TIMEOUT` is the
    /// deadline `check_head_freshness` now passes to
    /// `rev_parse_commit_with_deadline` instead of the 60s
    /// `gitdiff::GIT_TIMEOUT` default (see that function's call site above);
    /// the client's matching value is `frontend/src/lib/api.ts`'s
    /// `FETCH_TIMEOUT_MS = 40_000`, which isn't reachable from a Rust test,
    /// so it's asserted here as a documented literal instead.
    #[test]
    fn submit_timeout_hierarchy_is_internal_lt_server_lt_client() {
        const CLIENT_FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(40);
        assert!(
            HEAD_FRESHNESS_TIMEOUT < REQUEST_TIMEOUT,
            "rev-parse deadline must resolve before the server gives up on the whole request"
        );
        assert!(
            REQUEST_TIMEOUT < CLIENT_FETCH_TIMEOUT,
            "server request timeout must resolve before the client's fetch abort, or the \
             browser gives up first and can show failure for a submit the server still commits"
        );
    }

    /// `check_head_freshness` must resolve `HEAD` correctly using the short
    /// deadline, not just fail fast — a real (fast) rev-parse under 10s
    /// still succeeds and lets a matching submit through. This is the same
    /// repo/state setup `submit_same_id_replays_200_even_after_head_moved`
    /// uses (a real `repo_root` is required; `check_head_freshness` is a
    /// no-op without one).
    #[tokio::test]
    async fn check_head_freshness_short_deadline_still_resolves_matching_head() {
        let td = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let st = std::process::Command::new("git")
                .current_dir(td.path())
                .args(args)
                .output()
                .unwrap();
            assert!(
                st.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&st.stderr)
            );
        };
        git(&["init", "-b", "main"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(td.path().join("a.txt"), "one\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "base"]);
        let head_oid = crate::gitdiff::rev_parse_commit(td.path(), "HEAD").unwrap();

        let state = build_state_with_repo(td.path().to_path_buf(), head_oid);
        assert!(
            check_head_freshness(&state).await.is_none(),
            "a matching HEAD must not be refused under the short deadline"
        );
    }
}
