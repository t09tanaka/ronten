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

/// Resolves the real `git` binary's absolute path via `command -v git`,
/// using the test process's own (unmodified) `PATH`. Needed by
/// [`fake_git_shim`] so its shim script can still pass non-intercepted
/// invocations through to the genuine binary after `PATH` has been
/// overridden (for the spawned `ronten` process only) to prefer the fake
/// `git` ahead of the real one.
#[cfg(unix)]
fn real_git_path() -> String {
    let out = Command::new("sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    let path = String::from_utf8(out.stdout).unwrap().trim().to_string();
    assert!(!path.is_empty(), "could not resolve real git on PATH");
    path
}

/// Writes an executable POSIX shell shim named `git` into a fresh tempdir.
/// Any invocation whose argument list contains `match_substr` as a substring
/// is intercepted: the shim prints the `FAKE_STDERR` env var to its own
/// stderr and exits 128 without running real git at all. Every other
/// invocation is passed straight through to the real `git` binary named by
/// the `REAL_GIT` env var (see [`real_git_path`]).
///
/// This exists to simulate git-internal failures — permission errors,
/// `safe.directory` rejections, a corrupt repository — that P1-13 requires
/// be classified as `GitFailed` rather than `NotARepo`/`BadBase`, without
/// constructing an actually-corrupt or permission-denied repository (fragile
/// and platform-dependent). Every git call the test under test doesn't care
/// about behaves exactly like real git, so only the one intercepted call
/// deviates from a normal review run.
#[cfg(unix)]
fn fake_git_shim(match_substr: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let script = format!(
        "#!/bin/sh\ncase \"$*\" in\n  *'{match_substr}'*)\n    printf '%s\\n' \"$FAKE_STDERR\" >&2\n    exit 128\n    ;;\nesac\nexec \"$REAL_GIT\" \"$@\"\n"
    );
    let path = dir.path().join("git");
    std::fs::write(&path, script).unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    dir
}

/// Like [`expect_exit`], but lets the caller set extra environment variables
/// on the spawned `ronten` process (used to prepend a fake-git shim
/// directory onto `PATH` and to pass it its `REAL_GIT`/`FAKE_STDERR`
/// parameters).
#[cfg(unix)]
fn expect_exit_with_env(dir: &Path, args: &[&str], envs: &[(&str, &str)]) -> i32 {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ronten"));
    cmd.current_dir(dir).args(args);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let out = cmd.output().unwrap();
    assert!(out.stdout.is_empty(), "stdout must be empty on error paths");
    out.status.code().unwrap()
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
        "mutation_id": "it-full-draft",
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
    assert_eq!(result["version"], 3);
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

/// Concerns matching [`fixture_repo_with_opaque_files`]: `edit`/`add` cover
/// the plain-text files exactly like [`CONCERNS`], plus `opaque` claiming
/// each opaque file whole (a path-only location with no `start`/`end`), so
/// none of them fall into the `_unmapped` bucket.
const CONCERNS_WITH_OPAQUE: &str = r#"{"version":1,"concerns":[
  {"id":"edit","title":"Edit a.txt","risk":"low","locations":[{"path":"a.txt"}]},
  {"id":"add","title":"Add b.txt","risk":"medium","locations":[{"path":"b.txt"}]},
  {"id":"opaque","title":"Opaque files","risk":"high","locations":[
    {"path":"binary.bin"},{"path":"data.dat"},{"path":"huge.txt"}]}]}"#;

/// Like [`fixture_repo`], plus a binary file modified, a Git-LFS pointer
/// file modified, and a >1MB text file added — one of each opaque-content
/// kind `result_v3_lists_files_with_omission_reasons` needs to see reflected
/// in the result JSON's `files[]`.
fn fixture_repo_with_opaque_files() -> tempfile::TempDir {
    let td = tempfile::tempdir().unwrap();
    let d = td.path();
    git(d, &["init", "-b", "main"]);
    git(d, &["config", "user.email", "t@example.com"]);
    git(d, &["config", "user.name", "t"]);
    std::fs::write(d.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    std::fs::write(d.join("binary.bin"), b"\x00\x01\x02old-bytes").unwrap();
    std::fs::write(
        d.join("data.dat"),
        "version https://git-lfs.github.com/spec/v1\noid sha256:aaaa\nsize 100\n",
    )
    .unwrap();
    git(d, &["add", "."]);
    git(d, &["commit", "-m", "base"]);
    git(d, &["checkout", "-b", "feature"]);
    std::fs::write(d.join("a.txt"), "one\nTWO\nthree\nfour\n").unwrap();
    std::fs::write(d.join("b.txt"), "new file\n").unwrap();
    std::fs::write(d.join("binary.bin"), b"\x00\x01\x02new-bytes-here").unwrap();
    std::fs::write(
        d.join("data.dat"),
        "version https://git-lfs.github.com/spec/v1\noid sha256:bbbb\nsize 200\n",
    )
    .unwrap();
    std::fs::write(d.join("huge.txt"), "x".repeat(1_048_576 + 1)).unwrap();
    git(d, &["add", "."]);
    git(d, &["commit", "-m", "change"]);
    std::fs::write(d.join("concerns.json"), CONCERNS_WITH_OPAQUE).unwrap();
    td
}

/// Looks up a `GET /session` file entry by path (either side) and returns
/// its server-computed `id`.
fn file_id_for(session: &serde_json::Value, path: &str) -> String {
    session["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["new_path"] == path || f["old_path"] == path)
        .unwrap_or_else(|| panic!("no file with path {path} in session payload"))["id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// P1-1: the result JSON must let a standalone reader reconstruct which
/// files existed and what was omitted from rendering, without re-running the
/// diff — `files[]` covers a plain text file (rendered) alongside binary,
/// Git-LFS-pointer, and too-large files (each not rendered, with the
/// matching `omission_reason`).
#[test]
fn result_v3_lists_files_with_omission_reasons() {
    let td = fixture_repo_with_opaque_files();
    let (child, url) = spawn_review(td.path(), &[]);
    let api = api_base(&url);

    let (status, session) = common::get_json(&format!("{api}/session"));
    assert_eq!(status, 200);
    let ack_ids: Vec<serde_json::Value> = ["binary.bin", "data.dat", "huge.txt"]
        .iter()
        .map(|p| serde_json::Value::String(file_id_for(&session, p)))
        .collect();

    let draft = serde_json::json!({
        "revision": 0,
        "mutation_id": "it-omission",
        "draft": {
            "concerns": {
                "edit": {"verdict": "approve", "comments": []},
                "add": {"verdict": "approve", "comments": []},
                "opaque": {"verdict": "approve", "comments": []}
            },
            "general_comments": [],
            "acknowledgements": ack_ids
        }
    });
    let (status, body) = submit_raw(&url, draft);
    assert_eq!(status, 200, "body: {body}");

    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let result: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let files = result["files"].as_array().unwrap();

    let find = |path: &str| -> &serde_json::Value {
        files
            .iter()
            .find(|f| f["new_path"] == path || f["old_path"] == path)
            .unwrap_or_else(|| panic!("no file audit entry for {path}: {files:?}"))
    };

    let a = find("a.txt");
    assert_eq!(a["content_kind"], "text");
    assert_eq!(a["rendered"], true);
    assert!(a["omission_reason"].is_null());

    let binary = find("binary.bin");
    assert_eq!(binary["content_kind"], "binary");
    assert_eq!(binary["rendered"], false);
    assert_eq!(binary["omission_reason"], "binary");

    let lfs = find("data.dat");
    assert_eq!(lfs["content_kind"], "text");
    assert_eq!(lfs["rendered"], false);
    assert_eq!(lfs["omission_reason"], "lfs_pointer");

    let huge = find("huge.txt");
    assert_eq!(huge["content_kind"], "too-large");
    assert_eq!(huge["rendered"], false);
    assert_eq!(huge["omission_reason"], "too_large");

    // Every file audit entry carries a stable id matching the session
    // payload's own file ids.
    for path in ["a.txt", "b.txt", "binary.bin", "data.dat", "huge.txt"] {
        assert_eq!(
            find(path)["file_id"],
            serde_json::Value::String(file_id_for(&session, path)),
            "file_id mismatch for {path}"
        );
    }
}

/// P1-1: acknowledging a required file must be recorded in the result JSON
/// with the file id, the server-computed reasons acknowledgement was
/// required, and a timestamp — so a standalone reader can tell what was
/// acked and why without re-deriving the ack policy itself.
#[test]
fn result_v3_records_acknowledgements_with_reasons() {
    let td = fixture_repo_with_opaque_files();
    let (child, url) = spawn_review(td.path(), &[]);
    let api = api_base(&url);

    let (status, session) = common::get_json(&format!("{api}/session"));
    assert_eq!(status, 200);
    let binary_id = file_id_for(&session, "binary.bin");
    let lfs_id = file_id_for(&session, "data.dat");
    let huge_id = file_id_for(&session, "huge.txt");

    let draft = serde_json::json!({
        "revision": 0,
        "mutation_id": "it-ack",
        "draft": {
            "concerns": {
                "edit": {"verdict": "approve", "comments": []},
                "add": {"verdict": "approve", "comments": []},
                "opaque": {"verdict": "approve", "comments": []}
            },
            "general_comments": [],
            "acknowledgements": [binary_id.clone(), lfs_id.clone(), huge_id.clone()]
        }
    });
    let (status, body) = submit_raw(&url, draft);
    assert_eq!(status, 200, "body: {body}");

    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let result: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let acks = result["acknowledgements"].as_array().unwrap();
    assert_eq!(acks.len(), 3, "acknowledgements: {acks:?}");

    let find = |id: &str| -> &serde_json::Value {
        acks.iter()
            .find(|a| a["file_id"] == id)
            .unwrap_or_else(|| panic!("no acknowledgement for file id {id}: {acks:?}"))
    };

    let binary_ack = find(&binary_id);
    assert_eq!(binary_ack["reasons"], serde_json::json!(["opaque-content"]));
    let acked_at = binary_ack["acknowledged_at"].as_str().unwrap();
    assert!(
        acked_at.contains('T'),
        "acknowledged_at must be RFC3339: {acked_at}"
    );
    // Recorded at submit time (no per-ack tracking), same instant across
    // every acknowledgement in this submit.
    assert_eq!(acked_at, result["submitted_at"]);

    let lfs_ack = find(&lfs_id);
    assert_eq!(lfs_ack["reasons"], serde_json::json!(["lfs-pointer"]));

    let huge_ack = find(&huge_id);
    assert_eq!(huge_ack["reasons"], serde_json::json!(["opaque-content"]));

    // Files never acknowledged (a.txt, b.txt) must not appear.
    assert!(
        !acks
            .iter()
            .any(|a| a["file_id"] == file_id_for(&session, "a.txt")),
        "a.txt was never acknowledged and must not appear: {acks:?}"
    );
}

/// P1-1: `--dirty-policy ignore` must not silently claim the worktree was
/// clean — the result records that the check never ran.
#[test]
fn result_v3_worktree_ignore_is_not_clean() {
    let td = fixture_repo();
    let (child, url) = spawn_review(td.path(), &["--dirty-policy", "ignore"]);

    let (status, _) = submit_raw(&url, full_draft("approve"));
    assert_eq!(status, 200);

    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let result: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let worktree = &result["worktree"];
    assert_eq!(worktree["policy"], "ignore");
    assert_eq!(
        worktree["checked_at_start"], false,
        "ignore policy must never claim the worktree was checked: {worktree}"
    );
    assert_eq!(
        worktree["clean_at_start"], false,
        "an unchecked worktree must never be recorded as clean: {worktree}"
    );
}

/// P1-1: the worktree is re-checked at submit (not just at session start),
/// so a change that happened mid-review is reflected in the audit too.
#[test]
fn result_v3_worktree_rechecked_at_submit() {
    let td = fixture_repo();
    std::fs::write(td.path().join("a.txt"), "one\nTWO\nthree\nfour\nfive\n").unwrap();
    let (child, url) = spawn_review(td.path(), &["--dirty-policy", "warn"]);

    let (status, _) = submit_raw(&url, full_draft("approve"));
    assert_eq!(status, 200);

    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let result: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let worktree = &result["worktree"];
    assert_eq!(worktree["policy"], "warn");
    assert_eq!(worktree["checked_at_start"], true);
    assert_eq!(worktree["clean_at_start"], false);
    assert_eq!(
        worktree["checked_at_submit"], true,
        "submit must re-run the worktree check under warn policy: {worktree}"
    );
    // The dirty file is untouched between start and submit, so the re-check
    // agrees with the start-time result.
    assert_eq!(worktree["clean_at_submit"], false);
}

/// Task 5.4 (P1-9 middle / P1-12 identity): `build.rs` embeds enough about
/// the specific binary that produced a result (source commit, dirty flag,
/// frontend digest, rustc version, target triple, build profile) that two
/// builds both claiming `ronten_version` 0.1.0 stay distinguishable.
/// `target`/`profile` come from the `TARGET`/`PROFILE` build-script env vars,
/// which cargo always sets, so they must be populated for *any* build of
/// this test binary. `source_commit` needs a git checkout, which this repo
/// (the tree the test binary is built from) always is.
#[test]
fn result_v3_build_identity_is_populated() {
    let td = fixture_repo();
    let (child, url) = spawn_review(td.path(), &[]);

    let (status, _) = submit_raw(&url, full_draft("approve"));
    assert_eq!(status, 200);

    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let result: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let build = &result["build"];
    assert_eq!(build["ronten_version"], env!("CARGO_PKG_VERSION"));
    assert!(
        build["target"].is_string(),
        "target must always be set from the TARGET build-script env: {build}"
    );
    assert!(
        build["profile"].is_string(),
        "profile must always be set from the PROFILE build-script env: {build}"
    );
    assert!(
        build["source_commit"].is_string(),
        "this repo is a git checkout, so source_commit must be populated: {build}"
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
        "mutation_id": "it-request-changes",
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

/// The `--out` file must be written even when stdout is closed. This test
/// closes the parent's read end of the child's stdout pipe right after
/// spawning, so the child's later write to stdout hits a closed pipe
/// (`EPIPE`, surfaced by the Rust runtime as `io::ErrorKind::BrokenPipe`
/// rather than a `SIGPIPE` kill). `--out` is written first and does not
/// depend on stdout succeeding, so the result must still land on disk, the
/// process must still exit with the decision code, and it must not panic.
///
/// This test manages the child's stderr itself (rather than via
/// `spawn_review`'s shared helper) because it needs the *complete* stderr
/// output — including anything written after the banner — to assert the
/// absence of a panic message; the shared helper's background drain thread
/// discards that tail.
#[cfg(unix)]
#[test]
fn out_written_even_if_stdout_closed() {
    use std::io::BufRead;
    use std::io::Read as _;

    let td = fixture_repo();
    let mut child = Command::new(env!("CARGO_BIN_EXE_ronten"))
        .current_dir(td.path())
        .args([
            "review",
            "--base",
            "main",
            "--concerns",
            "concerns.json",
            "--no-open",
            "--out",
            "result.json",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    // Drop our end of the stdout pipe now, before the child ever writes to
    // it: once this end is closed, the child's write hits a broken pipe.
    drop(child.stdout.take());

    let stderr = child.stderr.take().unwrap();
    let mut reader = std::io::BufReader::new(stderr);
    let mut line = String::new();
    let mut stderr_so_far = String::new();
    let url = loop {
        line.clear();
        assert!(
            reader.read_line(&mut line).unwrap() > 0,
            "process exited before printing URL"
        );
        stderr_so_far.push_str(&line);
        if let Some(rest) = line.trim().strip_prefix("Review session: ") {
            break rest.to_string();
        }
    };

    let (status, _) = common::post_json(
        &format!("{}/submit", api_base(&url)),
        &full_draft("approve"),
    );
    assert_eq!(status, 200);

    // Read the rest of stderr to EOF: this both captures any later output
    // (e.g. a would-be panic message) and blocks until the child exits,
    // since EOF on this pipe only happens when the child closes it.
    let mut rest = String::new();
    reader.read_to_string(&mut rest).unwrap();
    stderr_so_far.push_str(&rest);

    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(0), "stderr: {stderr_so_far}");

    let file = std::fs::read_to_string(td.path().join("result.json")).unwrap();
    let result: serde_json::Value = serde_json::from_str(&file).unwrap();
    assert_eq!(result["decision"], "approve");

    assert!(
        !stderr_so_far.to_lowercase().contains("panic"),
        "stdout being closed must not panic: {stderr_so_far}"
    );
}

#[test]
fn out_reservation_failure_exits_15_before_start() {
    // The parent directory of `--out` doesn't exist. Since `--out` is now
    // reserved (atomically, via `create_new`) before the server ever starts,
    // this failure surfaces at that reservation step rather than at the
    // final write: the process exits with OUT_FAILED before printing the
    // session URL or accepting a submission at all, so stdout stays empty
    // (there is no review outcome to carry) — unlike the late-write-failure
    // path (see `out_late_write_failure_exits_15_with_stdout_intact` below),
    // where the outcome was already decided and only the final write failed.
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

/// Companion to `out_reservation_failure_exits_15_before_start`, covering the
/// other (reachable) way to hit `OUT_FAILED`: a write failure *after* the
/// session has already started and a decision has been reached. The
/// reservation placeholder is created successfully (so the session starts
/// normally and accepts a submission), but by the time `write_out_atomic`
/// renames its temp file over the reserved path at submit time, that path has
/// been replaced with a directory — the rename fails, `--out` is not written,
/// and the process exits `OUT_FAILED` (15). Unlike the pre-start reservation
/// failure, the review outcome already exists by then, so stdout must still
/// carry the full result JSON: `--out` failing must never cost the caller the
/// only other copy of the decision.
#[test]
fn out_late_write_failure_exits_15_with_stdout_intact() {
    let td = fixture_repo();
    let (child, url) = spawn_review(td.path(), &["--out", "result.json"]);
    assert!(
        td.path().join("result.json").exists(),
        "the placeholder must be reserved before the session is reachable over HTTP"
    );

    // Swap the reserved placeholder for a directory: `write_out_atomic`'s
    // final `rename(tmp, path)` fails when `path` is a (non-empty-target)
    // directory, deterministically reproducing the post-submit write
    // failure without relying on any timing race.
    std::fs::remove_file(td.path().join("result.json")).unwrap();
    std::fs::create_dir(td.path().join("result.json")).unwrap();

    let (status, _) = common::post_json(
        &format!("{}/submit", api_base(&url)),
        &full_draft("approve"),
    );
    assert_eq!(status, 200);

    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(15));
    let stdout = String::from_utf8(out.stdout).unwrap();
    let result: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("stdout must still carry the result JSON: {e}: {stdout:?}"));
    assert_eq!(result["decision"], "approve");
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

/// `git status`'s entry-count cap (Task 3.2, P0-6) must fail closed: once
/// the worktree has more untracked entries than the cap, the dirty gate
/// stops enumerating individual paths and instead blocks with a summary,
/// exactly like an ordinary dirty worktree (exit 17) — never silently
/// "clean" just because enumeration was cut short.
#[test]
fn status_overflow_blocks_as_dirty() {
    let td = fixture_repo();
    let dir = td.path().join("many");
    std::fs::create_dir(&dir).unwrap();
    // One entry past `STATUS_MAX_ENTRIES` (10_000) in src/gitdiff.rs.
    for i in 0..10_001 {
        std::fs::write(dir.join(format!("f{i}.txt")), "").unwrap();
    }

    let (code, stderr) = expect_exit_with_stderr(td.path(), &[]);
    assert_eq!(code, 17, "stderr: {stderr}");
    assert!(
        stderr.contains("more than 10000 entries"),
        "stderr must summarize the overflow instead of (or in addition to) enumerating every path: {stderr}"
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

/// P1-13: a `git rev-parse --show-toplevel` failure whose stderr is a
/// `safe.directory`-style rejection (not "not a git repository") must not be
/// misreported as "not a repo" (exit 12) — it's a git-internal problem, and
/// exits with `GIT_FAILED` instead.
#[cfg(unix)]
#[test]
fn safe_directory_or_permission_error_is_git_failed_not_12() {
    let td = tempfile::tempdir().unwrap();
    std::fs::write(td.path().join("concerns.json"), CONCERNS).unwrap();
    let shim_dir = fake_git_shim("--show-toplevel");
    let real_git = real_git_path();
    let path_env = format!(
        "{}:{}",
        shim_dir.path().display(),
        std::env::var("PATH").unwrap()
    );
    let code = expect_exit_with_env(
        td.path(),
        &[
            "review",
            "--base",
            "main",
            "--concerns",
            "concerns.json",
            "--no-open",
        ],
        &[
            ("PATH", path_env.as_str()),
            ("REAL_GIT", real_git.as_str()),
            (
                "FAKE_STDERR",
                "fatal: detected dubious ownership in repository at '/repo'\nTo add an exception for this directory, call:\n\n\tgit config --global --add safe.directory /repo",
            ),
        ],
    );
    assert_ne!(
        code, 12,
        "a safe.directory-style rejection must not be reported as exit 12 (not a repo)"
    );
    assert_eq!(code, 14, "expected GIT_FAILED exit code, got {code}");
}

/// P1-13: a base-ref resolution failure whose stderr is NOT a "bad ref"
/// signature (unknown/bad revision, ambiguous argument) must not be
/// misclassified as `BadBase` (exit 11) — it's a git-internal problem during
/// resolution and exits with `GIT_FAILED` instead.
#[cfg(unix)]
#[test]
fn git_internal_base_failure_is_git_failed() {
    let td = fixture_repo();
    let shim_dir = fake_git_shim("git-internal-failure-marker^{commit}");
    let real_git = real_git_path();
    let path_env = format!(
        "{}:{}",
        shim_dir.path().display(),
        std::env::var("PATH").unwrap()
    );
    let code = expect_exit_with_env(
        td.path(),
        &[
            "review",
            "--base",
            "git-internal-failure-marker",
            "--concerns",
            "concerns.json",
            "--no-open",
        ],
        &[
            ("PATH", path_env.as_str()),
            ("REAL_GIT", real_git.as_str()),
            (
                "FAKE_STDERR",
                "fatal: unable to read current working directory: Permission denied",
            ),
        ],
    );
    assert_ne!(
        code, 11,
        "a git-internal failure during base resolution must not be reported as exit 11 (bad base)"
    );
    assert_eq!(code, 14, "expected GIT_FAILED exit code, got {code}");
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

/// `review`'s startup validation and `ronten validate-concerns` share the
/// same `mapping::validate_concerns` semantic validator (see the P1-3 fix):
/// this fixture drives an invalid concerns file through both entry points
/// and asserts the *same* failure — the reserved `_unmapped` id — surfaces
/// from each, just formatted differently (`review` prints one human-readable
/// stderr line; `validate-concerns` emits a structured `errors` array).
#[test]
fn review_and_validate_concerns_agree_on_reserved_id() {
    let td = fixture_repo();
    let reserved = r#"{"version":1,"concerns":[
      {"id":"_unmapped","title":"nope","risk":"low","locations":[{"path":"a.txt"}]}]}"#;
    std::fs::write(td.path().join("reserved.json"), reserved).unwrap();

    // `ronten validate-concerns`: structured, machine-readable.
    let validate_out = Command::new(env!("CARGO_BIN_EXE_ronten"))
        .current_dir(td.path())
        .args(["validate-concerns", "reserved.json"])
        .output()
        .unwrap();
    assert_eq!(validate_out.status.code(), Some(10));
    let v: serde_json::Value = serde_json::from_slice(&validate_out.stdout).unwrap();
    assert_eq!(v["valid"], false);
    let errors = v["errors"].as_array().unwrap();
    assert!(
        errors
            .iter()
            .any(|e| e["code"] == "RESERVED_CONCERN_ID" && e["concern_id"] == "_unmapped"),
        "validate-concerns should report RESERVED_CONCERN_ID for _unmapped: {errors:?}"
    );

    // `ronten review`: same validator, human-readable stderr, same exit code.
    let review_out = Command::new(env!("CARGO_BIN_EXE_ronten"))
        .current_dir(td.path())
        .args([
            "review",
            "--base",
            "main",
            "--concerns",
            "reserved.json",
            "--no-open",
        ])
        .output()
        .unwrap();
    assert_eq!(review_out.status.code(), Some(10));
    let stderr = String::from_utf8(review_out.stderr).unwrap();
    assert!(
        stderr.contains("_unmapped") && stderr.contains("reserved"),
        "review's stderr should describe the same reserved-id failure: {stderr}"
    );
}

/// The concerns file is untrusted input, and `validate_concerns` interpolates
/// a location's `path` straight into its error message. A `path` carrying a
/// raw ESC byte (plus a newline, for good measure) must not reach stderr
/// unescaped — that would let the concerns file forge extra terminal/log
/// lines or ANSI-inject the rest of the output. This is the cleanest trigger
/// for that print site: `start: 0` is rejected by `validate_concerns` (line
/// numbers are 1-based) before any diff mapping happens, so the message is
/// guaranteed to include the offending `path` verbatim.
#[test]
fn concerns_error_output_escapes_control_chars() {
    let td = fixture_repo();
    let evil = "{\"version\":1,\"concerns\":[\
      {\"id\":\"edit\",\"title\":\"Edit\",\"risk\":\"low\",\"locations\":\
      [{\"path\":\"a.txt\\u001bevil\\ninjected\",\"start\":0}]}]}";
    std::fs::write(td.path().join("evil.json"), evil).unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_ronten"))
        .current_dir(td.path())
        .args([
            "review",
            "--base",
            "main",
            "--concerns",
            "evil.json",
            "--no-open",
        ])
        .output()
        .unwrap();
    assert!(out.stdout.is_empty(), "stdout must be empty on error paths");
    assert_eq!(out.status.code(), Some(10));
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("⟨U+001B⟩"),
        "stderr must escape the raw ESC byte from the concerns path as a visible token: {stderr}"
    );
    assert!(
        stderr.contains("⟨U+000A⟩"),
        "stderr must escape the embedded newline from the concerns path as a visible token: {stderr}"
    );
    assert!(
        !stderr.contains('\x1b'),
        "stderr must not contain a raw, unescaped ESC byte: {stderr:?}"
    );
    // The path's own embedded "\n" must not survive as a real newline: every
    // line that mentions the concerns error is the one line printed by
    // `eprintln!`, not split into extra forged lines by the raw byte.
    let matching_lines: Vec<&str> = stderr
        .lines()
        .filter(|l| l.contains("invalid concerns"))
        .collect();
    assert_eq!(
        matching_lines.len(),
        1,
        "the embedded newline must not have forged an extra log line: {stderr:?}"
    );
}

/// Companion to `concerns_error_output_escapes_control_chars` covering the
/// other raw-bytes-reach-stderr path: `deny_unknown_fields` embeds the
/// offending field name verbatim in serde's error `Display`, and that error
/// is printed at the JSON-parse site (`invalid concerns JSON: ...`), a
/// different print site than `validate_concerns`'s. A control character in an
/// unknown field name must be escaped there too.
#[test]
fn concerns_json_unknown_field_error_escapes_control_chars() {
    let td = fixture_repo();
    let evil = "{\"version\":1,\"concerns\":[\
      {\"id\":\"edit\",\"title\":\"Edit\",\"risk\":\"low\",\"locations\":[],\
      \"evil\\u001bfield\":true}]}";
    std::fs::write(td.path().join("evil_field.json"), evil).unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_ronten"))
        .current_dir(td.path())
        .args([
            "review",
            "--base",
            "main",
            "--concerns",
            "evil_field.json",
            "--no-open",
        ])
        .output()
        .unwrap();
    assert!(out.stdout.is_empty(), "stdout must be empty on error paths");
    assert_eq!(out.status.code(), Some(10));
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(
        stderr.contains("⟨U+001B⟩"),
        "stderr must escape the raw ESC byte from the unknown field name as a visible token: {stderr}"
    );
    assert!(
        !stderr.contains('\x1b'),
        "stderr must not contain a raw, unescaped ESC byte: {stderr:?}"
    );
}
