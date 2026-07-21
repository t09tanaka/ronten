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

/// 16 random bytes, rendered as lowercase hex — used as the session's
/// unguessable URL token.
pub fn new_token() -> String {
    let bytes: [u8; 16] = rand::rng().random();
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

    let draft = crate::session::lock_ignore_poison(&state.draft).clone();
    let submitted = state.is_finished();
    let payload = SessionPayload {
        title: &state.title,
        summary: state.summary.as_deref(),
        files: &state.files,
        concerns,
        unmapped_lines: &state.mapping.unmapped_lines,
        warnings: &state.mapping.warnings,
        draft,
        submitted,
    };
    Json(payload).into_response()
}

async fn put_draft(
    State(state): State<Arc<SessionState>>,
    Path(token): Path<String>,
    Json(draft): Json<Draft>,
) -> Response {
    if token != state.token {
        return not_found();
    }
    *crate::session::lock_ignore_poison(&state.draft) = draft;
    StatusCode::NO_CONTENT.into_response()
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
    let resolved =
        tokio::task::spawn_blocking(move || crate::gitdiff::rev_parse_commit(&root, "HEAD")).await;
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
    Json(draft): Json<Draft>,
) -> Response {
    if token != state.token {
        return not_found();
    }

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

    // Build the result first, then claim the terminal state: `try_finish`
    // stores the outcome and claims the terminal in one atomic step, so
    // there is no window where the session is finished but its outcome is
    // unreadable. Claimed only after full validation, so a rejected submit
    // never consumes the session.
    let result = state.build_result(&draft);
    if !state.try_finish(Outcome::Submitted(Box::new(result))) {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error": "already submitted"})),
        )
            .into_response();
    }
    Json(json!({"ok": true})).into_response()
}

async fn post_abort(State(state): State<Arc<SessionState>>, Path(token): Path<String>) -> Response {
    if token != state.token {
        return not_found();
    }

    if !state.try_finish(Outcome::Aborted) {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error": "already submitted"})),
        )
            .into_response();
    }
    Json(json!({"ok": true})).into_response()
}

pub fn build_router(state: Arc<SessionState>) -> Router {
    Router::new()
        .route("/r/{token}", get(get_index))
        .route("/assets/{*path}", get(get_asset))
        .route("/api/{token}/session", get(get_session))
        .route("/api/{token}/draft", put(put_draft))
        .route("/api/{token}/submit", post(post_submit))
        .route("/api/{token}/abort", post(post_abort))
        .with_state(state)
        .layer(middleware::from_fn(security_headers))
        .layer(middleware::from_fn_with_state(
            REQUEST_TIMEOUT,
            request_timeout,
        ))
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
            draft: std::sync::Mutex::new(Draft::default()),
            phase: std::sync::Mutex::new(crate::session::Phase::Reviewing),
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
            draft: std::sync::Mutex::new(Draft::default()),
            phase: std::sync::Mutex::new(crate::session::Phase::Reviewing),
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
            old_oid: Some("1111111111111111111111111111111111111111".to_string()),
            new_oid: Some("2222222222222222222222222222222222222222".to_string()),
            old_size: Some(10),
            new_size: Some(20),
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
            draft: std::sync::Mutex::new(Draft::default()),
            phase: std::sync::Mutex::new(crate::session::Phase::Reviewing),
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
        assert_eq!(body["submitted"], false);
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
            "concerns": { "c1": { "verdict": "approve", "comments": [] } },
            "general_comments": []
        });
        let (status, _) = call(
            app.clone(),
            put_json(&format!("/api/{TOKEN}/draft"), draft_body),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);

        let (status, body) = call(app, get(&format!("/api/{TOKEN}/session"))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["draft"]["concerns"]["c1"]["verdict"], "approve");
    }

    #[tokio::test]
    async fn draft_roundtrip_acknowledged_opaque() {
        let state = build_opaque_state();
        let app = build_router(state.clone());

        let draft_body = json!({
            "concerns": {},
            "general_comments": [],
            "acknowledged_opaque": [1]
        });
        let (status, _) = call(
            app.clone(),
            put_json(&format!("/api/{TOKEN}/draft"), draft_body),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);

        let (status, body) = call(app, get(&format!("/api/{TOKEN}/session"))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["draft"]["acknowledged_opaque"], json!([1]));
    }

    #[tokio::test]
    async fn submit_incomplete_422() {
        let state = build_state();
        let app = build_router(state.clone());

        let draft_body = json!({
            "concerns": { "c1": { "verdict": "approve", "comments": [] } },
            "general_comments": []
        });
        let (status, body) =
            call(app, post_json(&format!("/api/{TOKEN}/submit"), draft_body)).await;
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
        let (status, body) =
            call(app, post_json(&format!("/api/{TOKEN}/submit"), draft_body)).await;
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
        let (status, body) = call(app, post_json(&format!("/api/{TOKEN}/submit"), draft)).await;
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
        let (status, body) = call(app, post_json(&format!("/api/{TOKEN}/submit"), draft)).await;
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
            post_json(&format!("/api/{TOKEN}/submit"), draft_body.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        state.finished_outcome().unwrap();

        let (status, body) =
            call(app, post_json(&format!("/api/{TOKEN}/submit"), draft_body)).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"], "already submitted");
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
        let (status, body) = call(app, post_json(&format!("/api/{TOKEN}/submit"), draft)).await;
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
        let (status, body) = call(app, post_json(&format!("/api/{TOKEN}/submit"), draft)).await;
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
        let (status, body) =
            call(app.clone(), post_json(&format!("/api/{TOKEN}/submit"), bad)).await;
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
        let (status, body) = call(app, post_json(&format!("/api/{TOKEN}/submit"), good)).await;
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
        let (status, body) = call(app, post_json(&format!("/api/{TOKEN}/submit"), draft)).await;
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
        let (status, body) = call(app, post_json(&format!("/api/{TOKEN}/submit"), draft)).await;
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
        let (status, body) = call(app, post_json(&format!("/api/{TOKEN}/submit"), draft)).await;
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
                    post_json(&format!("/api/{TOKEN}/submit"), complete_draft()),
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
    /// holding its connection open forever.
    #[tokio::test]
    async fn overrunning_request_gets_408() {
        let app = Router::new()
            .route(
                "/slow",
                axum::routing::get(|| async {
                    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                    "too late"
                }),
            )
            .layer(middleware::from_fn_with_state(
                std::time::Duration::from_millis(20),
                request_timeout,
            ));
        let (status, body) = call(app, get("/slow")).await;
        assert_eq!(status, StatusCode::REQUEST_TIMEOUT);
        assert_eq!(body["error"], "request timed out");
    }
}
