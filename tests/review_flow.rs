//! End-to-end integration tests for `ronten review`.
//!
//! Each test builds its own git fixture repo in a tempdir and spawns a real
//! `ronten review` process, driving it over HTTP (via `ureq`) and asserting on
//! its exit code and stdout. Integration tests cannot import `src/` internals,
//! so the git fixture helper is duplicated from `gitdiff.rs`'s `git_tests`.

use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, Stdio};

/// Concerns JSON matching the fixture repo: `edit` covers a.txt, `add` covers
/// b.txt. Written to `<repo>/concerns.json` (untracked, so absent from the diff).
const CONCERNS: &str = r#"{"version":1,"concerns":[
  {"id":"edit","title":"Edit a.txt","risk":"low","locations":[{"path":"a.txt"}]},
  {"id":"add","title":"Add b.txt","risk":"medium","locations":[{"path":"b.txt"}]}]}"#;

fn git(dir: &Path, args: &[&str]) {
    let st = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        st.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&st.stderr)
    );
}

/// A git repo on branch `feature` with a.txt modified and b.txt added relative
/// to `main`, plus a `concerns.json` file (not committed).
fn fixture_repo() -> tempfile::TempDir {
    let td = tempfile::tempdir().unwrap();
    let d = td.path();
    git(d, &["init", "-b", "main"]);
    git(d, &["config", "user.email", "t@example.com"]);
    git(d, &["config", "user.name", "t"]);
    std::fs::write(d.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    git(d, &["add", "."]);
    git(d, &["commit", "-m", "base"]);
    git(d, &["checkout", "-b", "feature"]);
    std::fs::write(d.join("a.txt"), "one\nTWO\nthree\nfour\n").unwrap();
    std::fs::write(d.join("b.txt"), "new file\n").unwrap();
    git(d, &["add", "."]);
    git(d, &["commit", "-m", "change"]);
    std::fs::write(d.join("concerns.json"), CONCERNS).unwrap();
    td
}

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

/// Spawns `ronten review --base main --concerns concerns.json --no-open`
/// (plus `extra`), waits for the session URL, and returns `(child, url)`.
fn spawn_review(dir: &Path, extra: &[&str]) -> (Child, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ronten"))
        .current_dir(dir)
        .args([
            "review",
            "--base",
            "main",
            "--concerns",
            "concerns.json",
            "--no-open",
        ])
        .args(extra)
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

fn full_draft(verdict: &str) -> serde_json::Value {
    serde_json::json!({
        "concerns": {
            "edit": {"verdict": verdict, "comments": []},
            "add": {"verdict": "approve", "comments": []}
        },
        "general_comments": []
    })
}

#[test]
fn approve_all_exits_0() {
    let td = fixture_repo();
    let (child, url) = spawn_review(td.path(), &[]);

    let resp = ureq::post(&format!("{}/submit", api_base(&url)))
        .send_json(full_draft("approve"))
        .unwrap();
    assert_eq!(resp.status(), 200);

    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let result: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(result["decision"], "approve");
    assert_eq!(result["concerns"].as_array().unwrap().len(), 2);
    // The result carries the contract version ronten processed.
    assert_eq!(result["version"], 1);
}

#[test]
fn request_changes_exits_1() {
    let td = fixture_repo();
    let (child, url) = spawn_review(td.path(), &[]);

    let draft = serde_json::json!({
        "concerns": {
            "edit": {"verdict": "request-changes", "comments": [
                {"path": "a.txt", "side": "new", "line": 2, "body": "please fix TWO"}
            ]},
            "add": {"verdict": "approve", "comments": []}
        },
        "general_comments": []
    });
    let resp = ureq::post(&format!("{}/submit", api_base(&url)))
        .send_json(draft)
        .unwrap();
    assert_eq!(resp.status(), 200);

    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    let result: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(result["decision"], "request-changes");
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("please fix TWO"),
        "inline comment missing from stdout: {stdout}"
    );
}

#[test]
fn abort_exits_2() {
    let td = fixture_repo();
    let (child, url) = spawn_review(td.path(), &[]);

    let resp = ureq::post(&format!("{}/abort", api_base(&url)))
        .call()
        .unwrap();
    assert_eq!(resp.status(), 200);

    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty(), "stdout must be empty on abort");
}

#[test]
fn timeout_exits_3() {
    let td = fixture_repo();
    // 1s deadline, no interaction: the process must exit on its own.
    let (child, _url) = spawn_review(td.path(), &["--timeout", "1s"]);

    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(3));
    assert!(out.stdout.is_empty(), "stdout must be empty on timeout");
}

#[test]
fn out_flag_writes_file() {
    let td = fixture_repo();
    let (child, url) = spawn_review(td.path(), &["--out", "result.json"]);

    let resp = ureq::post(&format!("{}/submit", api_base(&url)))
        .send_json(full_draft("approve"))
        .unwrap();
    assert_eq!(resp.status(), 200);

    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let file = std::fs::read_to_string(td.path().join("result.json")).unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    // stdout is pretty JSON + trailing newline from println!; file is the same
    // pretty JSON without the trailing newline.
    assert_eq!(file, stdout.trim_end_matches('\n'));
}

#[test]
fn concerns_from_stdin() {
    let td = fixture_repo();
    let mut child = Command::new(env!("CARGO_BIN_EXE_ronten"))
        .current_dir(td.path())
        .args(["review", "--base", "main", "--concerns", "-", "--no-open"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(CONCERNS.as_bytes())
        .unwrap();

    let url = read_review_url(&mut child);
    let resp = ureq::get(&format!("{}/session", api_base(&url)))
        .call()
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Clean up: abort and wait for exit.
    ureq::post(&format!("{}/abort", api_base(&url)))
        .call()
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn session_serves_html() {
    let td = fixture_repo();
    let (child, url) = spawn_review(td.path(), &[]);

    let resp = ureq::get(&url).call().unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp.content_type().starts_with("text/html"));
    let body = resp.into_string().unwrap();
    assert!(
        body.contains(r#"<div id="app">"#),
        "index body missing app root div: {body}"
    );

    ureq::post(&format!("{}/abort", api_base(&url)))
        .call()
        .unwrap();
    let _ = child.wait_with_output().unwrap();
}

/// Spawns `ronten review` expecting an immediate exit (no session) and returns
/// the exit code.
fn expect_exit(dir: &Path, args: &[&str]) -> i32 {
    let out = Command::new(env!("CARGO_BIN_EXE_ronten"))
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(out.stdout.is_empty(), "stdout must be empty on error paths");
    out.status.code().unwrap()
}

#[test]
fn bad_base_exits_11() {
    let td = fixture_repo();
    let code = expect_exit(
        td.path(),
        &[
            "review",
            "--base",
            "no-such-ref",
            "--concerns",
            "concerns.json",
            "--no-open",
        ],
    );
    assert_eq!(code, 11);
}

#[test]
fn not_a_repo_exits_12() {
    let td = tempfile::tempdir().unwrap();
    std::fs::write(td.path().join("concerns.json"), CONCERNS).unwrap();
    let code = expect_exit(
        td.path(),
        &[
            "review",
            "--base",
            "main",
            "--concerns",
            "concerns.json",
            "--no-open",
        ],
    );
    assert_eq!(code, 12);
}

#[test]
fn empty_diff_exits_13() {
    let td = fixture_repo();
    let code = expect_exit(
        td.path(),
        &[
            "review",
            "--base",
            "HEAD",
            "--concerns",
            "concerns.json",
            "--no-open",
        ],
    );
    assert_eq!(code, 13);
}

#[test]
fn invalid_concerns_exits_10() {
    let td = fixture_repo();
    std::fs::write(td.path().join("bad.json"), "{not valid json").unwrap();
    let code = expect_exit(
        td.path(),
        &[
            "review",
            "--base",
            "main",
            "--concerns",
            "bad.json",
            "--no-open",
        ],
    );
    assert_eq!(code, 10);
}

#[test]
fn oversized_concerns_input_is_rejected() {
    // 8MiB 超の concerns JSON は読み込み段階で拒否される（メモリ保護）。
    let td = fixture_repo();
    // Leading whitespace pads the byte count past the 8MiB cap without
    // touching any field-level length limit (summary/title/etc.), so this
    // exercises the raw-size cap specifically, not per-field validation.
    let padding = " ".repeat(9 * 1024 * 1024);
    let huge = format!(
        r#"{padding}{{"version":1,"concerns":[
      {{"id":"edit","title":"Edit a.txt","risk":"low","locations":[{{"path":"a.txt"}}]}}]}}"#
    );
    std::fs::write(td.path().join("huge.json"), &huge).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_ronten"))
        .current_dir(td.path())
        .args([
            "review",
            "--base",
            "main",
            "--concerns",
            "huge.json",
            "--no-open",
        ])
        .output()
        .unwrap();
    assert!(out.stdout.is_empty(), "stdout must be empty on error paths");
    assert_eq!(out.status.code(), Some(10));
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("exceeds") || stderr.to_lowercase().contains("size"),
        "stderr missing size-exceeded message: {stderr}"
    );
}

#[test]
fn unsupported_version_exits_10() {
    let td = fixture_repo();
    let v2 = r#"{"version":2,"concerns":[
      {"id":"edit","title":"Edit a.txt","risk":"low","locations":[{"path":"a.txt"}]}]}"#;
    std::fs::write(td.path().join("v2.json"), v2).unwrap();
    let code = expect_exit(
        td.path(),
        &[
            "review",
            "--base",
            "main",
            "--concerns",
            "v2.json",
            "--no-open",
        ],
    );
    assert_eq!(code, 10);
}

#[test]
fn reserved_id_exits_10() {
    let td = fixture_repo();
    let reserved = r#"{"version":1,"concerns":[
      {"id":"_unmapped","title":"nope","risk":"low","locations":[{"path":"a.txt"}]}]}"#;
    std::fs::write(td.path().join("reserved.json"), reserved).unwrap();
    let code = expect_exit(
        td.path(),
        &[
            "review",
            "--base",
            "main",
            "--concerns",
            "reserved.json",
            "--no-open",
        ],
    );
    assert_eq!(code, 10);
}
