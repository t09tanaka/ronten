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
/// banner and returns the URL. Panics if the process exits first. Delegates
/// to [`read_review_url_with_prebanner`] and discards the pre-banner text;
/// see that function for the draining behavior after the banner is found.
fn read_review_url(child: &mut Child) -> String {
    read_review_url_with_prebanner(child).0
}

/// Spawns `ronten review --base main --concerns concerns.json --no-open`
/// (plus `extra`), waits for the session URL, and returns `(child, url)`.
/// Delegates to [`spawn_review_with_prebanner`] and discards the pre-banner
/// text.
fn spawn_review(dir: &Path, extra: &[&str]) -> (Child, String) {
    let (child, url, _prebanner) = spawn_review_with_prebanner(dir, extra);
    (child, url)
}

/// Reads the child's stderr line by line until the `Review session: <url>`
/// banner, returning the URL together with every stderr line seen before it
/// (joined back together) — the dirty-worktree warning (if any) prints
/// before the banner, so this is how tests observe it. Panics if the process
/// exits first.
///
/// After the banner is found, a background thread keeps draining the pipe to
/// EOF rather than dropping the read end here: the child may still write to
/// stderr later (e.g. an `--out` write failure warning), and dropping our end
/// of the pipe would make that write hit a broken pipe and panic the child.
fn read_review_url_with_prebanner(child: &mut Child) -> (String, String) {
    use std::io::BufRead;
    let stderr = child.stderr.take().unwrap();
    let mut reader = std::io::BufReader::new(stderr);
    let mut line = String::new();
    let mut prebanner = String::new();
    let url = loop {
        line.clear();
        assert!(
            reader.read_line(&mut line).unwrap() > 0,
            "process exited before printing URL"
        );
        if let Some(rest) = line.trim().strip_prefix("Review session: ") {
            break rest.to_string();
        }
        prebanner.push_str(&line);
    };
    std::thread::spawn(move || {
        let mut sink = String::new();
        while reader.read_line(&mut sink).unwrap_or(0) > 0 {
            sink.clear();
        }
    });
    (url, prebanner)
}

/// Spawns `ronten review --base main --concerns concerns.json --no-open`
/// (plus `extra`) and returns `(child, url, prebanner)`, where `prebanner` is
/// every stderr line seen before the `Review session: ` banner (see
/// [`read_review_url_with_prebanner`]).
fn spawn_review_with_prebanner(dir: &Path, extra: &[&str]) -> (Child, String, String) {
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
    let (url, prebanner) = read_review_url_with_prebanner(&mut child);
    (child, url, prebanner)
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

    // Atomicity regression: the write goes through a same-directory temp
    // file that is renamed into place, so no `.tmp.<pid>` sibling should
    // ever survive a successful write.
    let leftovers: Vec<_> = std::fs::read_dir(td.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|name| name.starts_with("result.json.tmp."))
        .collect();
    assert!(
        leftovers.is_empty(),
        "temp file(s) left behind after atomic write: {leftovers:?}"
    );
}

#[test]
fn out_write_failure_exits_15_with_stdout_intact() {
    // The parent directory of `--out` doesn't exist, so the atomic
    // write must fail — but the review outcome already happened, so
    // stdout must still carry the correct result JSON, only the exit
    // code changes (to the dedicated OUT_FAILED code), regardless of the
    // approve/request-changes decision.
    let td = fixture_repo();
    let (child, url) = spawn_review(td.path(), &["--out", "no-such-dir/result.json"]);

    let resp = ureq::post(&format!("{}/submit", api_base(&url)))
        .send_json(full_draft("approve"))
        .unwrap();
    assert_eq!(resp.status(), 200);

    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(15));
    let result: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(result["decision"], "approve");
    assert!(
        !td.path().join("no-such-dir").exists(),
        "the missing parent directory must not have been created as a side effect"
    );
}

#[test]
fn dirty_tracked_file_prints_uncommitted_changes_warning() {
    // `concerns.json` in the fixture is already untracked (excluded via
    // -uno); modify a *tracked* file without committing so the warning must
    // fire.
    let td = fixture_repo();
    std::fs::write(td.path().join("a.txt"), "one\nTWO\nthree\nfour\nfive\n").unwrap();

    let (child, url, prebanner) = spawn_review_with_prebanner(td.path(), &[]);
    assert!(
        prebanner.contains(
            "warning: tracked files have uncommitted changes; this review covers committed state only (main...HEAD)"
        ),
        "stderr missing dirty-worktree warning: {prebanner}"
    );

    ureq::post(&format!("{}/abort", api_base(&url)))
        .call()
        .unwrap();
    let _ = child.wait_with_output().unwrap();
}

#[test]
fn empty_diff_with_dirty_tracked_file_exits_13_with_warning() {
    // Regression test: the empty-diff early return used to happen before the
    // dirty-worktree check, so the single most important case — an agent
    // that committed nothing at all, leaving every change uncommitted — never
    // got the warning. `--base feature` against HEAD (also `feature`) is an
    // empty diff by construction; then a tracked file is modified without
    // committing.
    let td = fixture_repo();
    std::fs::write(td.path().join("a.txt"), "one\nTWO\nthree\nfour\nfive\n").unwrap();

    let child = Command::new(env!("CARGO_BIN_EXE_ronten"))
        .current_dir(td.path())
        .args([
            "review",
            "--base",
            "feature",
            "--concerns",
            "concerns.json",
            "--no-open",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(13));
    assert!(out.stdout.is_empty(), "stdout must be empty on empty diff");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("nothing to review"),
        "stderr missing empty-diff message: {stderr}"
    );
    assert!(
        stderr.contains(
            "warning: tracked files have uncommitted changes; this review covers committed state only (feature...HEAD)"
        ),
        "stderr missing dirty-worktree warning: {stderr}"
    );
}

#[test]
fn clean_worktree_prints_no_uncommitted_changes_warning() {
    let td = fixture_repo();
    let (child, url, prebanner) = spawn_review_with_prebanner(td.path(), &[]);
    assert!(
        !prebanner.contains("tracked files have uncommitted changes"),
        "unexpected dirty-worktree warning on a clean worktree: {prebanner}"
    );

    ureq::post(&format!("{}/abort", api_base(&url)))
        .call()
        .unwrap();
    let _ = child.wait_with_output().unwrap();
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
