//! Token-protected localhost HTTP server exposing the review session.

use crate::assets;
use crate::model::ResultOutput;
use crate::session::{Draft, SessionPayload, SessionState, Terminal};
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use rand::Rng;
use serde_json::json;
use std::sync::Arc;

/// What a session ended with; sent once over `outcome_tx` when the review
/// finishes. `Timeout` is never produced by the HTTP handlers here — it's
/// emitted later by the review loop's `select!` arm when the deadline fires
/// before a submit/abort arrives.
#[derive(Debug)]
pub enum Outcome {
    Submitted(ResultOutput),
    Aborted,
    Timeout,
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

    let draft = state.draft.lock().unwrap().clone();
    let submitted = state.finished.lock().unwrap().is_some();
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
    *state.draft.lock().unwrap() = draft;
    StatusCode::NO_CONTENT.into_response()
}

async fn post_submit(
    State(state): State<Arc<SessionState>>,
    Path(token): Path<String>,
    Json(draft): Json<Draft>,
) -> Response {
    if token != state.token {
        return not_found();
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

    // Claim the single terminal state only after full validation, so a
    // rejected submit never consumes the session.
    if !state.try_finish(Terminal::Submitted) {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error": "already submitted"})),
        )
            .into_response();
    }

    let result = state.build_result(&draft);
    let _ = state.outcome_tx.send(Outcome::Submitted(result)).await;
    Json(json!({"ok": true})).into_response()
}

async fn post_abort(State(state): State<Arc<SessionState>>, Path(token): Path<String>) -> Response {
    if token != state.token {
        return not_found();
    }

    if !state.try_finish(Terminal::Aborted) {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error": "already submitted"})),
        )
            .into_response();
    }

    let _ = state.outcome_tx.send(Outcome::Aborted).await;
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

    fn build_state() -> (Arc<SessionState>, tokio::sync::mpsc::Receiver<Outcome>) {
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

        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let state = Arc::new(SessionState {
            title: "review title".to_string(),
            summary: Some("summary text".to_string()),
            files,
            mapping,
            input,
            token: TOKEN.to_string(),
            started_at: chrono::Utc::now(),
            draft: std::sync::Mutex::new(Draft::default()),
            finished: std::sync::Mutex::new(None),
            outcome_tx: tx,
        });
        (state, rx)
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

    fn build_rename_state() -> (Arc<SessionState>, tokio::sync::mpsc::Receiver<Outcome>) {
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

        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let state = Arc::new(SessionState {
            title: "rename review".to_string(),
            summary: None,
            files,
            mapping,
            input,
            token: TOKEN.to_string(),
            started_at: chrono::Utc::now(),
            draft: std::sync::Mutex::new(Draft::default()),
            finished: std::sync::Mutex::new(None),
            outcome_tx: tx,
        });
        (state, rx)
    }

    /// Binary ファイルを1つ含む state（c1 が whole-file location で claim）。
    fn build_opaque_state() -> (Arc<SessionState>, tokio::sync::mpsc::Receiver<Outcome>) {
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

        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let state = Arc::new(SessionState {
            title: "opaque review".to_string(),
            summary: None,
            files,
            mapping,
            input,
            token: TOKEN.to_string(),
            started_at: chrono::Utc::now(),
            draft: std::sync::Mutex::new(Draft::default()),
            finished: std::sync::Mutex::new(None),
            outcome_tx: tx,
        });
        (state, rx)
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
        let (state, _rx) = build_state();
        let app = build_router(state);
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
        let (state, _rx) = build_state();
        let app = build_router(state);
        let (status, body) = call(app, get("/api/deadbeef/session")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body.is_null());
    }

    #[tokio::test]
    async fn draft_roundtrip() {
        let (state, _rx) = build_state();
        let app = build_router(state);

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
        let (state, _rx) = build_opaque_state();
        let app = build_router(state);

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
        let (state, _rx) = build_state();
        let app = build_router(state);

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
        let (state, mut rx) = build_state();
        let app = build_router(state);

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

        let outcome = rx.recv().await.unwrap();
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
    async fn second_submit_409() {
        let (state, mut rx) = build_state();
        let app = build_router(state);

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
        rx.recv().await.unwrap();

        let (status, body) =
            call(app, post_json(&format!("/api/{TOKEN}/submit"), draft_body)).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"], "already submitted");
    }

    #[tokio::test]
    async fn abort_emits_outcome() {
        let (state, mut rx) = build_state();
        let app = build_router(state);

        let (status, body) = call(app, post_empty(&format!("/api/{TOKEN}/abort"))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], true);

        let outcome = rx.recv().await.unwrap();
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
        let (state, _rx) = build_state();
        let app = build_router(state);
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
        let (state, mut rx) = build_state();
        let app = build_router(state);

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

        let outcome = rx.recv().await.unwrap();
        match outcome {
            Outcome::Submitted(r) => {
                assert_eq!(r.version, 1);
                assert_eq!(r.concerns[0].comments.len(), 3);
            }
            other => panic!("expected Submitted, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn submit_old_side_against_new_path_422_then_old_path_200() {
        let (state, mut rx) = build_rename_state();
        let app = build_router(state);

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
        assert!(matches!(rx.recv().await.unwrap(), Outcome::Submitted(_)));
    }

    #[tokio::test]
    async fn submit_without_opaque_ack_422() {
        let (state, _rx) = build_opaque_state();
        let app = build_router(state);
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
        let (state, mut rx) = build_opaque_state();
        let app = build_router(state);
        let draft = json!({
            "concerns": { "c1": { "verdict": "approve", "comments": [] } },
            "general_comments": [],
            "acknowledged_opaque": [1]
        });
        let (status, body) = call(app, post_json(&format!("/api/{TOKEN}/submit"), draft)).await;
        assert_eq!(status, StatusCode::OK, "body: {body}");
        assert!(matches!(rx.recv().await.unwrap(), Outcome::Submitted(_)));
    }

    #[tokio::test]
    async fn submit_ack_on_non_opaque_file_422() {
        let (state, _rx) = build_opaque_state();
        let app = build_router(state);
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

        let (state, _rx) = build_state();
        let app = build_router(state);
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
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        assert!(!bytes.is_empty(), "expected non-empty asset body");
    }
}
