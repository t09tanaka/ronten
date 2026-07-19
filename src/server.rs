//! Token-protected localhost HTTP server exposing the review session.

use crate::assets;
use crate::model::ResultOutput;
use crate::session::{Draft, SessionPayload, SessionState};
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
    #[allow(dead_code)]
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
    assets::serve(&path)
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
    let submitted = *state.finished.lock().unwrap();
    let payload = SessionPayload {
        title: &state.title,
        summary: state.summary.as_deref(),
        files: &state.files,
        concerns,
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

    let result = {
        let mut finished = state.finished.lock().unwrap();
        if *finished {
            return (
                StatusCode::CONFLICT,
                Json(json!({"error": "already submitted"})),
            )
                .into_response();
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

        *finished = true;
        state.build_result(&draft)
    };

    let _ = state.outcome_tx.send(Outcome::Submitted(result)).await;
    Json(json!({"ok": true})).into_response()
}

async fn post_abort(State(state): State<Arc<SessionState>>, Path(token): Path<String>) -> Response {
    if token != state.token {
        return not_found();
    }

    {
        let mut finished = state.finished.lock().unwrap();
        if *finished {
            return (
                StatusCode::CONFLICT,
                Json(json!({"error": "already submitted"})),
            )
                .into_response();
        }
        *finished = true;
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
    use crate::gitdiff::parse_unified_diff;
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
            finished: std::sync::Mutex::new(false),
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
                "_unmapped": { "verdict": "comment", "comments": [] }
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
}
