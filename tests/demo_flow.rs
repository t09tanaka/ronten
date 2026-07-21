//! Integration test for `ronten demo`: spawns the real binary and drives it
//! over HTTP, same style as `tests/review_flow.rs`. Integration tests can't
//! share code across test files, so the small stderr-URL helper is
//! duplicated here.

use std::process::{Child, Command, Stdio};

mod common;

/// Reads the child's stderr line by line until the `Review session: <url>`
/// banner and returns the URL. Panics if the process exits first.
fn read_review_url(child: &mut Child) -> String {
    use std::io::BufRead;
    let stderr = child.stderr.take().unwrap();
    let mut reader = std::io::BufReader::new(stderr);
    let mut line = String::new();
    loop {
        line.clear();
        assert!(
            reader.read_line(&mut line).unwrap() > 0,
            "process exited before printing URL"
        );
        if let Some(rest) = line.trim().strip_prefix("Review session: ") {
            return rest.to_string();
        }
    }
}

/// Spawns `ronten demo --no-open`, waits for the session URL, and returns
/// `(child, url)`.
fn spawn_demo() -> (Child, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ronten"))
        .args(["demo", "--no-open"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let url = read_review_url(&mut child);
    (child, url)
}

/// `http://127.0.0.1:PORT/r/TOKEN` → `http://127.0.0.1:PORT/api/TOKEN`.
fn api_base(url: &str) -> String {
    let (origin, token) = url.rsplit_once("/r/").unwrap();
    format!("{origin}/api/{token}")
}

#[test]
fn session_has_four_concerns_including_unmapped() {
    let (child, url) = spawn_demo();

    let (status, body) = common::get_json(&format!("{}/session", api_base(&url)));
    assert_eq!(status, 200);

    assert_eq!(body["title"], "ronten demo");
    let concerns = body["concerns"].as_array().unwrap();
    assert_eq!(
        concerns.len(),
        4,
        "expected 3 declared concerns + _unmapped, got {concerns:?}"
    );
    let ids: Vec<&str> = concerns.iter().map(|c| c["id"].as_str().unwrap()).collect();
    assert_eq!(
        ids,
        vec!["auth-core", "route-wiring", "logging", "_unmapped"]
    );
    assert_eq!(concerns[3]["unmapped"], true);

    // The demo diff must actually parse into files with hunks.
    let files = body["files"].as_array().unwrap();
    assert_eq!(files.len(), 3);

    common::post_empty(&format!("{}/abort", api_base(&url)));
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn abort_exits_2() {
    let (child, url) = spawn_demo();

    let (status, _) = common::post_empty(&format!("{}/abort", api_base(&url)));
    assert_eq!(status, 200);

    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty(), "stdout must be empty on abort");
}
