//! End-to-end integration tests for `ronten review`.
//!
//! Each test builds its own git fixture repo in a tempdir and spawns a real
//! `ronten review` process, driving it over HTTP (via `ureq`) and asserting on
//! its exit code and stdout. Integration tests cannot import `src/` internals,
//! so the git fixture helper is duplicated from `gitdiff.rs`'s `git_tests`.

use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, Stdio};

mod common;

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

/// Complete submit body (wire shape `{revision, draft}`) at revision 0 —
/// these tests never save a draft first.
fn full_draft(verdict: &str) -> serde_json::Value {
    serde_json::json!({
        "revision": 0,
        "draft": {
            "concerns": {
                "edit": {"verdict": verdict, "comments": []},
                "add": {"verdict": "approve", "comments": []}
            },
            "general_comments": []
        }
    })
}

#[test]
fn approve_all_exits_0() {
    let td = fixture_repo();
    let (child, url) = spawn_review(td.path(), &[]);

    let (status, _) = common::post_json(
        &format!("{}/submit", api_base(&url)),
        &full_draft("approve"),
    );
    assert_eq!(status, 200);

    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let result: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(result["decision"], "approve");
    assert_eq!(result["concerns"].as_array().unwrap().len(), 2);
    // The result carries the output contract version.
    assert_eq!(result["version"], 2);
}

/// stdout of `git <args>` in `dir`, trimmed.
fn git_stdout(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// The result JSON must pin the review to the exact commits and inputs it
/// covered: base/head/merge-base oids as resolved at session start (even if
/// the base ref moves afterwards), plus canonical digests and the advisory
/// assurance marker.
#[test]
fn result_pins_reviewed_commits_and_inputs() {
    let td = fixture_repo();
    let base_oid = git_stdout(td.path(), &["rev-parse", "main"]);
    let head_oid = git_stdout(td.path(), &["rev-parse", "feature"]);
    let (child, url) = spawn_review(td.path(), &[]);

    // Move the base ref after the session started: the result must keep the
    // oid resolved at start, not re-resolve the moved ref.
    git(td.path(), &["branch", "-f", "main", "feature"]);

    let (status, _) = common::post_json(
        &format!("{}/submit", api_base(&url)),
        &full_draft("approve"),
    );
    assert_eq!(status, 200);

    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let result: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let review = &result["review"];
    assert_eq!(review["base_ref"], "main");
    assert_eq!(review["base_oid"], serde_json::json!(base_oid));
    assert_eq!(review["head_oid"], serde_json::json!(head_oid));
    // merge-base(main, feature) is main itself in the linear fixture.
    assert_eq!(review["merge_base_oid"], serde_json::json!(base_oid));
    assert_eq!(review["assurance"], "advisory");
    assert_eq!(review["ronten_version"], env!("CARGO_PKG_VERSION"));
    for key in ["diff_sha256", "concerns_sha256"] {
        let digest = review[key].as_str().unwrap();
        assert_eq!(digest.len(), 64, "{key} must be a sha256 hex digest");
        assert!(digest.bytes().all(|b| b.is_ascii_hexdigit()));
    }
    assert!(
        !review["session_id"].as_str().unwrap().is_empty(),
        "session_id must be present"
    );
}

/// Posts `draft` to `/submit` and returns `(status, body)` without treating
/// non-2xx as a transport error.
fn submit_raw(url: &str, draft: serde_json::Value) -> (u16, serde_json::Value) {
    common::post_json(&format!("{}/submit", api_base(url)), &draft)
}

/// Advancing HEAD after the session started must make submit fail with 409
/// "review stale": the human approved the diff of the old HEAD, and that
/// approval must not be attachable to the new commit.
#[test]
fn head_advance_makes_submit_stale_409() {
    let td = fixture_repo();
    let (child, url) = spawn_review(td.path(), &[]);

    std::fs::write(td.path().join("a.txt"), "one\nTWO\nthree\nfour\nSNEAK\n").unwrap();
    git(td.path(), &["add", "a.txt"]);
    git(td.path(), &["commit", "-m", "sneaky extra commit"]);

    let (status, body) = submit_raw(&url, full_draft("approve"));
    assert_eq!(status, 409, "body: {body}");
    assert_eq!(body["error"], "review stale");
    assert!(
        body["details"].to_string().contains("HEAD changed"),
        "details missing HEAD-changed explanation: {body}"
    );

    // The stale session emits no result: abort it and expect the abort code.
    common::post_empty(&format!("{}/abort", api_base(&url)));
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(
        out.stdout.is_empty(),
        "stale session must not emit a result"
    );
}

/// Checking out a different branch mid-review must also 409: HEAD no longer
/// resolves to the reviewed commit.
#[test]
fn branch_switch_makes_submit_stale_409() {
    let td = fixture_repo();
    let (child, url) = spawn_review(td.path(), &[]);

    git(td.path(), &["checkout", "main"]);

    let (status, body) = submit_raw(&url, full_draft("approve"));
    assert_eq!(status, 409, "body: {body}");
    assert_eq!(body["error"], "review stale");

    // Switching back restores the reviewed commit, so submit succeeds again.
    git(td.path(), &["checkout", "feature"]);
    let (status, body) = submit_raw(&url, full_draft("approve"));
    assert_eq!(status, 200, "body: {body}");

    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn request_changes_exits_1() {
    let td = fixture_repo();
    let (child, url) = spawn_review(td.path(), &[]);

    let draft = serde_json::json!({
        "revision": 0,
        "draft": {
            "concerns": {
                "edit": {"verdict": "request-changes", "comments": [
                    {"path": "a.txt", "side": "new", "line": 2, "body": "please fix TWO"}
                ]},
                "add": {"verdict": "approve", "comments": []}
            },
            "general_comments": []
        }
    });
    let (status, _) = common::post_json(&format!("{}/submit", api_base(&url)), &draft);
    assert_eq!(status, 200);

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

    let (status, _) = common::post_empty(&format!("{}/abort", api_base(&url)));
    assert_eq!(status, 200);

    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty(), "stdout must be empty on abort");
}

/// Ctrl-C (SIGINT) with no submitted outcome must abort with exit code 2 and
/// an empty stdout, and must do so promptly — the shutdown path has a hard
/// deadline, so a signal can never leave the process hanging.
#[cfg(unix)]
#[test]
fn sigint_aborts_exits_2() {
    let td = fixture_repo();
    let (child, _url) = spawn_review(td.path(), &[]);

    let st = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(st.success());

    let start = std::time::Instant::now();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(
        out.stdout.is_empty(),
        "stdout must be empty on ctrl-c abort"
    );
    assert!(
        start.elapsed() < std::time::Duration::from_secs(10),
        "sigint shutdown took too long: {:?}",
        start.elapsed()
    );
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

    let (status, _) = common::post_json(
        &format!("{}/submit", api_base(&url)),
        &full_draft("approve"),
    );
    assert_eq!(status, 200);

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
    // The parent directory of `--out` doesn't exist. Since `--out` is now
    // reserved (atomically, via `create_new`) before the server ever starts,
    // this failure surfaces at that reservation step rather than at the
    // final write: the process exits with OUT_FAILED before printing the
    // session URL or accepting a submission at all, so stdout stays empty
    // (there is no review outcome to carry) — unlike the pre-reservation
    // design, where the outcome was already decided and only the final
    // write failed.
    let td = fixture_repo();
    let code = expect_exit(
        td.path(),
        &[
            "review",
            "--base",
            "main",
            "--concerns",
            "concerns.json",
            "--no-open",
            "--out",
            "no-such-dir/result.json",
        ],
    );
    assert_eq!(code, 15);
    assert!(
        !td.path().join("no-such-dir").exists(),
        "the missing parent directory must not have been created as a side effect"
    );
}

/// Spawns `ronten review` (with `--base main --concerns concerns.json
/// --no-open` plus `extra`) expecting an immediate exit — no session, no
/// stdout — and returns `(exit code, stderr)`. Companion to `expect_exit`
/// for the `--out` preflight-rejection tests below, which also need to
/// assert on the rejection message.
fn expect_exit_with_stderr(dir: &Path, extra: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_ronten"))
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
        .output()
        .unwrap();
    assert!(out.stdout.is_empty(), "stdout must be empty on error paths");
    (
        out.status.code().unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

/// A pre-existing file at the `--out` target must be refused (exit 15)
/// before the server ever starts: overwriting it would silently discard
/// whatever is there, most plausibly a result from a previous run.
#[test]
fn out_refuses_existing_target() {
    let td = fixture_repo();
    std::fs::write(
        td.path().join("result.json"),
        "leftover from a previous run\n",
    )
    .unwrap();

    let (code, stderr) = expect_exit_with_stderr(td.path(), &["--out", "result.json"]);
    assert_eq!(code, 15, "stderr: {stderr}");
    assert!(
        stderr.contains("already exists"),
        "stderr missing existing-target message: {stderr}"
    );
    assert!(
        stderr.contains("move or delete"),
        "stderr must tell the user to move/delete the stale result: {stderr}"
    );
    assert_eq!(
        std::fs::read_to_string(td.path().join("result.json")).unwrap(),
        "leftover from a previous run\n",
        "the pre-existing file must not have been touched"
    );
}

/// A `--out` target that is tracked by git must be refused: overwriting it
/// would blow away part of the reviewed repository, not just a scratch file.
#[test]
fn out_refuses_tracked_file() {
    let td = fixture_repo();

    let (code, stderr) = expect_exit_with_stderr(td.path(), &["--out", "a.txt"]);
    assert_eq!(code, 15, "stderr: {stderr}");
    assert!(
        stderr.contains("tracked"),
        "stderr missing tracked-file message: {stderr}"
    );
}

/// A `--out` target inside `.git` must be refused: writing there could
/// corrupt the repository's own bookkeeping.
#[test]
fn out_refuses_git_dir() {
    let td = fixture_repo();

    let (code, stderr) = expect_exit_with_stderr(td.path(), &["--out", ".git/result.json"]);
    assert_eq!(code, 15, "stderr: {stderr}");
    assert!(
        stderr.contains("git directory"),
        "stderr missing git-dir message: {stderr}"
    );
    assert!(
        !td.path().join(".git/result.json").exists(),
        "nothing should have been written inside .git"
    );
}

/// `--out` pointed at the same file as `--concerns` must be refused: ronten
/// would be reading and clobbering the same path in one run.
#[test]
fn out_refuses_concerns_path() {
    let td = fixture_repo();

    let (code, stderr) = expect_exit_with_stderr(td.path(), &["--out", "concerns.json"]);
    assert_eq!(code, 15, "stderr: {stderr}");
    assert!(
        stderr.contains("same file") && stderr.contains("--concerns"),
        "stderr missing same-as-concerns message: {stderr}"
    );
    assert_eq!(
        std::fs::read_to_string(td.path().join("concerns.json")).unwrap(),
        CONCERNS,
        "the concerns file must not have been touched"
    );
}

/// A `--out` target that is already a symlink must be refused, even a
/// dangling one — following it to write would escape the intended location.
#[cfg(unix)]
#[test]
fn out_refuses_symlink() {
    let td = fixture_repo();
    std::os::unix::fs::symlink("nowhere", td.path().join("result.json")).unwrap();

    let (code, stderr) = expect_exit_with_stderr(td.path(), &["--out", "result.json"]);
    assert_eq!(code, 15, "stderr: {stderr}");
    assert!(
        std::fs::symlink_metadata(td.path().join("result.json"))
            .unwrap()
            .file_type()
            .is_symlink(),
        "the symlink itself must be left alone, not replaced or removed"
    );
}

/// Aborting a session started with `--out` must remove the placeholder
/// reserved for it — otherwise a later run would see it as a stale existing
/// target and refuse to start (see `out_refuses_existing_target`).
#[test]
fn out_placeholder_removed_on_abort() {
    let td = fixture_repo();
    let (child, url) = spawn_review(td.path(), &["--out", "result.json"]);
    assert!(
        td.path().join("result.json").exists(),
        "the placeholder must be reserved before the session is reachable over HTTP"
    );

    let (status, _) = common::post_empty(&format!("{}/abort", api_base(&url)));
    assert_eq!(status, 200);

    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(
        !td.path().join("result.json").exists(),
        "the placeholder must be removed once the session aborts without a submission"
    );
}

/// A stale `result.json` left over from a previous run must block the
/// review from starting at all (exit 15, before the server binds), not just
/// fail once a decision is finally reached.
#[test]
fn out_stale_result_blocks_start() {
    let td = fixture_repo();
    std::fs::write(
        td.path().join("result.json"),
        r#"{"version":2,"decision":"approve"}"#,
    )
    .unwrap();

    let (code, stderr) = expect_exit_with_stderr(td.path(), &["--out", "result.json"]);
    assert_eq!(code, 15);
    assert!(
        stderr.contains("result.json"),
        "stderr must name the stale target: {stderr}"
    );
}

/// The default dirty policy is `error`: an uncommitted change to a tracked
/// file refuses to start the review with the dedicated exit code 17.
#[test]
fn dirty_tracked_file_blocks_start_by_default() {
    let td = fixture_repo();
    std::fs::write(td.path().join("a.txt"), "one\nTWO\nthree\nfour\nfive\n").unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_ronten"))
        .current_dir(td.path())
        .args([
            "review",
            "--base",
            "main",
            "--concerns",
            "concerns.json",
            "--no-open",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(17));
    assert!(out.stdout.is_empty(), "stdout must be empty on dirty error");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("worktree is not clean"),
        "stderr missing dirty-worktree error: {stderr}"
    );
    assert!(
        stderr.contains("uncommitted change (tracked): a.txt"),
        "stderr must name the dirty file: {stderr}"
    );
}

/// A brand-new file the agent forgot to `git add` is exactly the change a
/// review must not silently exclude: untracked files block by default.
#[test]
fn untracked_file_blocks_start_by_default() {
    let td = fixture_repo();
    std::fs::write(td.path().join("forgotten.rs"), "fn main() {}\n").unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_ronten"))
        .current_dir(td.path())
        .args([
            "review",
            "--base",
            "main",
            "--concerns",
            "concerns.json",
            "--no-open",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(17));
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("untracked file: forgotten.rs"),
        "stderr must name the untracked file: {stderr}"
    );
    assert!(
        !stderr.contains("concerns.json"),
        "the concerns file itself is exempt from the dirty gate: {stderr}"
    );
}

/// Unix filenames may contain bytes a terminal or log line would otherwise
/// interpret specially — here an ESC (start of an ANSI/OSC escape sequence)
/// and a raw newline (which could forge an extra, fake log line). The dirty
/// listing must escape both to the visible `⟨U+XXXX⟩` token instead of
/// echoing them verbatim to stderr.
#[cfg(unix)]
#[test]
fn dirty_listing_escapes_control_chars() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let td = fixture_repo();
    let name = OsStr::from_bytes(b"evil\x1binjected\nname.txt");
    std::fs::write(td.path().join(name), "danger\n").unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_ronten"))
        .current_dir(td.path())
        .args([
            "review",
            "--base",
            "main",
            "--concerns",
            "concerns.json",
            "--no-open",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(17));
    assert!(out.stdout.is_empty(), "stdout must be empty on dirty error");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("⟨U+001B⟩"),
        "stderr must escape the raw ESC byte as a visible token: {stderr}"
    );
    assert!(
        stderr.contains("⟨U+000A⟩"),
        "stderr must escape the embedded newline as a visible token: {stderr}"
    );
    assert!(
        !stderr.contains('\x1b'),
        "stderr must not contain a raw, unescaped ESC byte: {stderr:?}"
    );
}

/// The concerns file being ronten's own input does not exempt it from the
/// dirty gate once it is tracked: a tracked, uncommitted modification to
/// concerns.json must still block start, exactly like any other tracked
/// change. Only an *untracked* concerns file matching the `--concerns` path
/// exactly is exempt (see `untracked_file_blocks_start_by_default`).
#[test]
fn tracked_concerns_change_blocks_start() {
    let td = fixture_repo();
    git(td.path(), &["add", "concerns.json"]);
    git(td.path(), &["commit", "-m", "track concerns"]);
    // Uncommitted modification to the now-tracked concerns.json.
    std::fs::write(td.path().join("concerns.json"), format!("{CONCERNS}\n")).unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_ronten"))
        .current_dir(td.path())
        .args([
            "review",
            "--base",
            "main",
            "--concerns",
            "concerns.json",
            "--no-open",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(17));
    assert!(out.stdout.is_empty(), "stdout must be empty on dirty error");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("uncommitted change (tracked): concerns.json"),
        "stderr must name the dirty tracked concerns file: {stderr}"
    );
}

/// Regression for the canonicalize-based exemption this task replaces: the
/// old `drop_exempt` canonicalized *every* status entry's path and dropped
/// it if that resolved to the same file as the (canonicalized) concerns
/// path. A symlink aliasing a genuinely dirty tracked file to the concerns
/// argument would therefore have canonicalized to the same real path and
/// been silently dropped from `tracked_changes` too — hiding a real
/// uncommitted change.
///
/// Construction: `tracked.json` is a tracked file (valid concerns JSON) with
/// an uncommitted modification. `alias.json` is an untracked symlink to
/// `tracked.json`, passed as `--concerns alias.json`. Canonicalizing
/// `alias.json` resolves to the same real file as `tracked.json`, but the
/// new lexical repo-relative comparison only ever exempts an entry in
/// `untracked` whose path string is exactly "alias.json" (there is none —
/// `alias.json` resolves, canonicalized, to "tracked.json", which is what
/// gets compared, and "tracked.json" never appears in `untracked`). So the
/// dirty tracked change to `tracked.json` must still block start.
#[cfg(unix)]
#[test]
fn symlink_alias_does_not_exempt_tracked_change() {
    let td = fixture_repo();
    // The fixture's default untracked concerns.json is irrelevant to this
    // test (concerns come from alias.json here) and would itself block
    // start as an untracked file, muddying the assertion below — remove it.
    std::fs::remove_file(td.path().join("concerns.json")).unwrap();
    std::fs::write(td.path().join("tracked.json"), CONCERNS).unwrap();
    git(td.path(), &["add", "tracked.json"]);
    git(td.path(), &["commit", "-m", "track concerns target"]);
    // Uncommitted modification to the tracked file.
    std::fs::write(td.path().join("tracked.json"), format!("{CONCERNS}\n")).unwrap();
    std::os::unix::fs::symlink(td.path().join("tracked.json"), td.path().join("alias.json"))
        .unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_ronten"))
        .current_dir(td.path())
        .args([
            "review",
            "--base",
            "main",
            "--concerns",
            "alias.json",
            "--no-open",
        ])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(17),
        "the symlink-aliased tracked change must still block start"
    );
    assert!(out.stdout.is_empty(), "stdout must be empty on dirty error");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("uncommitted change (tracked): tracked.json"),
        "stderr must name the dirty tracked file, not silently drop it via the symlink alias: {stderr}"
    );
}

/// Regression for a second canonicalize hole: resolving the concerns path's
/// *own* final component (not just aliasing another status entry, as in
/// `symlink_alias_does_not_exempt_tracked_change` above) let a gitignored
/// symlink at the concerns path stand in for whatever it points at.
///
/// Construction: `.gitignore` (committed) ignores `concerns.json`.
/// `forgotten.rs` is an untracked file containing valid concerns JSON.
/// `concerns.json` is a symlink to `forgotten.rs` — itself gitignored, so
/// `git status` reports neither an untracked nor a tracked entry for
/// `concerns.json` at all, only `? forgotten.rs`. Passed as `--concerns
/// concerns.json`, the old (fully-canonicalizing) resolution would follow
/// the symlink and return "forgotten.rs" as the exempt repo-relative path —
/// removing the one real untracked entry `git status` reported, even though
/// `forgotten.rs` is an unrelated file the symlink merely happens to point
/// at. The fixed resolution never canonicalizes the leaf, so it returns
/// "concerns.json" (which never appears in `untracked`, since it's
/// gitignored) — `forgotten.rs` must still block start.
#[cfg(unix)]
#[test]
fn ignored_symlink_concerns_does_not_exempt_other_untracked() {
    let td = fixture_repo();
    // The fixture's default untracked concerns.json is irrelevant here
    // (concerns.json is redefined below as a gitignored symlink) — remove it
    // first so it doesn't shadow the construction.
    std::fs::remove_file(td.path().join("concerns.json")).unwrap();
    std::fs::write(td.path().join(".gitignore"), "concerns.json\n").unwrap();
    git(td.path(), &["add", ".gitignore"]);
    git(td.path(), &["commit", "-m", "ignore concerns.json"]);
    std::fs::write(td.path().join("forgotten.rs"), CONCERNS).unwrap();
    std::os::unix::fs::symlink(
        td.path().join("forgotten.rs"),
        td.path().join("concerns.json"),
    )
    .unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_ronten"))
        .current_dir(td.path())
        .args([
            "review",
            "--base",
            "main",
            "--concerns",
            "concerns.json",
            "--no-open",
        ])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(17),
        "the untracked file the gitignored symlink points at must still block start"
    );
    assert!(out.stdout.is_empty(), "stdout must be empty on dirty error");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("untracked file: forgotten.rs"),
        "stderr must name forgotten.rs as untracked, not silently exempt it via the gitignored symlink leaf: {stderr}"
    );
}

/// A concerns file that is tracked but has no uncommitted modifications must
/// not be reported as dirty (no false positive from the new comparison).
#[test]
fn tracked_unmodified_concerns_on_clean_worktree_still_starts() {
    let td = fixture_repo();
    git(td.path(), &["add", "concerns.json"]);
    git(td.path(), &["commit", "-m", "track concerns"]);

    let (child, url, prebanner) = spawn_review_with_prebanner(td.path(), &[]);
    assert!(
        !prebanner.contains("worktree is not clean"),
        "unexpected dirty-worktree warning for a clean, tracked concerns file: {prebanner}"
    );

    common::post_empty(&format!("{}/abort", api_base(&url)));
    let _ = child.wait_with_output().unwrap();
}

#[test]
fn dirty_tracked_file_with_warn_policy_prints_warning_and_starts() {
    let td = fixture_repo();
    std::fs::write(td.path().join("a.txt"), "one\nTWO\nthree\nfour\nfive\n").unwrap();

    let (child, url, prebanner) =
        spawn_review_with_prebanner(td.path(), &["--dirty-policy", "warn"]);
    assert!(
        prebanner.contains("warning: worktree is not clean"),
        "stderr missing dirty-worktree warning: {prebanner}"
    );
    assert!(
        prebanner.contains("uncommitted change (tracked): a.txt"),
        "warning must name the dirty file: {prebanner}"
    );

    common::post_empty(&format!("{}/abort", api_base(&url)));
    let _ = child.wait_with_output().unwrap();
}

#[test]
fn empty_diff_with_dirty_worktree_errors_before_empty_diff_report() {
    // An agent that committed nothing at all — every change uncommitted,
    // `--base feature` against HEAD (also `feature`) an empty diff by
    // construction — is the single most dangerous case. The dirty gate must
    // fire (exit 17) before the empty-diff report would.
    let td = fixture_repo();
    std::fs::write(td.path().join("a.txt"), "one\nTWO\nthree\nfour\nfive\n").unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_ronten"))
        .current_dir(td.path())
        .args([
            "review",
            "--base",
            "feature",
            "--concerns",
            "concerns.json",
            "--no-open",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(17));
    assert!(out.stdout.is_empty(), "stdout must be empty on dirty error");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("worktree is not clean"),
        "stderr missing dirty-worktree error: {stderr}"
    );
}

#[test]
fn empty_diff_with_dirty_worktree_and_warn_policy_exits_13_with_warning() {
    // With `--dirty-policy warn` the old behavior holds: the empty-diff
    // report still carries the dirty-worktree warning so the reviewer isn't
    // left staring at "nothing to review" with no clue.
    let td = fixture_repo();
    std::fs::write(td.path().join("a.txt"), "one\nTWO\nthree\nfour\nfive\n").unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_ronten"))
        .current_dir(td.path())
        .args([
            "review",
            "--base",
            "feature",
            "--concerns",
            "concerns.json",
            "--no-open",
            "--dirty-policy",
            "warn",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(13));
    assert!(out.stdout.is_empty(), "stdout must be empty on empty diff");
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("nothing to review"),
        "stderr missing empty-diff message: {stderr}"
    );
    assert!(
        stderr.contains("warning: worktree is not clean"),
        "stderr missing dirty-worktree warning: {stderr}"
    );
}

#[test]
fn clean_worktree_prints_no_uncommitted_changes_warning() {
    // The fixture's untracked `concerns.json` is exempt (it is ronten's own
    // input), so the default error policy still lets the session start.
    let td = fixture_repo();
    let (child, url, prebanner) = spawn_review_with_prebanner(td.path(), &[]);
    assert!(
        !prebanner.contains("worktree is not clean"),
        "unexpected dirty-worktree warning on a clean worktree: {prebanner}"
    );

    common::post_empty(&format!("{}/abort", api_base(&url)));
    let _ = child.wait_with_output().unwrap();
}

#[test]
fn concerns_from_stdin() {
    let td = fixture_repo();
    // Concerns come from stdin here, so the on-disk concerns.json is not
    // exempt from the dirty gate — remove it to keep the worktree clean.
    std::fs::remove_file(td.path().join("concerns.json")).unwrap();
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
    let (status, _) = common::get_json(&format!("{}/session", api_base(&url)));
    assert_eq!(status, 200);

    // Clean up: abort and wait for exit.
    common::post_empty(&format!("{}/abort", api_base(&url)));
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn session_serves_html() {
    let td = fixture_repo();
    let (child, url) = spawn_review(td.path(), &[]);

    let (status, content_type, body) = common::get_text(&url);
    assert_eq!(status, 200);
    assert!(content_type.starts_with("text/html"));
    assert!(
        body.contains(r#"<div id="app">"#),
        "index body missing app root div: {body}"
    );

    common::post_empty(&format!("{}/abort", api_base(&url)));
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
