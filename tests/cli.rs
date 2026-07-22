use assert_cmd::Command;
use std::io::Write;
use std::process::Stdio;

#[test]
fn help_lists_subcommands() {
    let out = Command::cargo_bin("ronten")
        .unwrap()
        .arg("--help")
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    for sub in ["review", "schema", "validate-concerns", "demo"] {
        assert!(stdout.contains(sub), "missing subcommand {sub} in help");
    }
}

#[test]
fn usage_error_exits_10_not_2() {
    // `review` without required args must exit 10 (2 is reserved for "aborted")
    Command::cargo_bin("ronten")
        .unwrap()
        .arg("review")
        .assert()
        .code(10);
}

#[test]
fn unknown_subcommand_exits_10() {
    Command::cargo_bin("ronten")
        .unwrap()
        .arg("bogus")
        .assert()
        .code(10);
}

#[test]
fn schema_prints_valid_json_with_both_contracts() {
    let out = Command::cargo_bin("ronten")
        .unwrap()
        .arg("schema")
        .assert()
        .success();
    let v: serde_json::Value =
        serde_json::from_slice(&out.get_output().stdout).expect("stdout must be pure JSON");
    assert!(v.get("input").is_some() && v.get("output").is_some());
}

#[test]
fn schema_input_only() {
    let out = Command::cargo_bin("ronten")
        .unwrap()
        .args(["schema", "--input"])
        .assert()
        .success();
    let v: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert!(
        v.get("input").is_none(),
        "--input prints the schema itself, not a wrapper"
    );
    assert_eq!(v["title"], "ConcernsInput");
}

/// A single valid concern (matches the CLI examples elsewhere in the repo).
const VALID_CONCERNS: &str = r#"{"version":1,"concerns":[
  {"id":"c1","title":"A concern","risk":"low","locations":[{"path":"a.ts"}]}]}"#;

#[test]
fn validate_concerns_valid_exits_0() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("concerns.json");
    std::fs::write(&path, VALID_CONCERNS).unwrap();

    let out = Command::cargo_bin("ronten")
        .unwrap()
        .args(["validate-concerns", path.to_str().unwrap()])
        .assert()
        .code(0);
    let v: serde_json::Value =
        serde_json::from_slice(&out.get_output().stdout).expect("stdout must be pure JSON");
    assert_eq!(v, serde_json::json!({"valid": true}));
}

#[test]
fn validate_concerns_invalid_exits_10_machine_readable() {
    // Duplicate concern ids: structurally valid JSON, semantically invalid.
    let dup = r#"{"version":1,"concerns":[
      {"id":"c1","title":"A","risk":"low"},
      {"id":"c1","title":"B","risk":"low"}]}"#;
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("concerns.json");
    std::fs::write(&path, dup).unwrap();

    let out = Command::cargo_bin("ronten")
        .unwrap()
        .args(["validate-concerns", path.to_str().unwrap()])
        .assert()
        .code(10);
    let v: serde_json::Value =
        serde_json::from_slice(&out.get_output().stdout).expect("stdout must be pure JSON");
    assert_eq!(v["valid"], false);
    let errors = v["errors"].as_array().expect("errors must be an array");
    assert!(
        errors
            .iter()
            .any(|e| e["code"] == "DUPLICATE_CONCERN_ID" && e["concern_id"] == "c1"),
        "expected a DUPLICATE_CONCERN_ID error for c1: {errors:?}"
    );
}

#[test]
fn validate_concerns_from_stdin() {
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_ronten"))
        .args(["validate-concerns", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(VALID_CONCERNS.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v, serde_json::json!({"valid": true}));
}

#[test]
fn validate_concerns_omitted_file_also_reads_stdin() {
    // Omitting the file argument entirely must behave like `-` (stdin), per
    // the same convention `review --concerns -` uses.
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_ronten"))
        .args(["validate-concerns"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(VALID_CONCERNS.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn validate_concerns_start_after_end() {
    let bad = r#"{"version":1,"concerns":[
      {"id":"c1","title":"A","risk":"low",
       "locations":[{"path":"a.ts","start":5,"end":4}]}]}"#;
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("concerns.json");
    std::fs::write(&path, bad).unwrap();

    let out = Command::cargo_bin("ronten")
        .unwrap()
        .args(["validate-concerns", path.to_str().unwrap()])
        .assert()
        .code(10);
    let v: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    let errors = v["errors"].as_array().unwrap();
    assert!(
        errors
            .iter()
            .any(|e| e["code"] == "START_AFTER_END" && e["concern_id"] == "c1"),
        "expected a START_AFTER_END error for c1: {errors:?}"
    );
}

#[test]
fn validate_concerns_duplicate_id() {
    let dup = r#"{"version":1,"concerns":[
      {"id":"dup","title":"A","risk":"low"},
      {"id":"dup","title":"B","risk":"low"}]}"#;
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("concerns.json");
    std::fs::write(&path, dup).unwrap();

    let out = Command::cargo_bin("ronten")
        .unwrap()
        .args(["validate-concerns", path.to_str().unwrap()])
        .assert()
        .code(10);
    let v: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    let errors = v["errors"].as_array().unwrap();
    assert!(
        errors
            .iter()
            .any(|e| e["code"] == "DUPLICATE_CONCERN_ID" && e["concern_id"] == "dup"),
        "expected a DUPLICATE_CONCERN_ID error for dup: {errors:?}"
    );
}

#[test]
fn validate_concerns_malformed_json_is_a_validation_error_not_a_panic() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("concerns.json");
    std::fs::write(&path, "{not valid json").unwrap();

    let out = Command::cargo_bin("ronten")
        .unwrap()
        .args(["validate-concerns", path.to_str().unwrap()])
        .assert()
        .code(10);
    let v: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(v["valid"], false);
    let errors = v["errors"].as_array().unwrap();
    assert!(errors.iter().any(|e| e["code"] == "INVALID_JSON"));
}

#[test]
fn validate_concerns_empty_stdin_is_a_validation_error_not_a_panic() {
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_ronten"))
        .args(["validate-concerns", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    // Close stdin immediately without writing anything.
    drop(child.stdin.take());
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(10));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["valid"], false);
}

#[test]
fn validate_concerns_missing_file_is_a_clear_error_not_a_panic() {
    let out = Command::cargo_bin("ronten")
        .unwrap()
        .args(["validate-concerns", "/no/such/path/concerns.json"])
        .assert()
        .code(10);
    let v: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(v["valid"], false);
    let errors = v["errors"].as_array().unwrap();
    assert!(errors.iter().any(|e| e["code"] == "READ_FAILED"));
}

#[test]
fn schema_version_is_a_const_and_timestamps_have_date_time_format() {
    let out = Command::cargo_bin("ronten")
        .unwrap()
        .arg("schema")
        .assert()
        .success();
    let v: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(v["input"]["properties"]["version"]["const"], 1);
    assert_eq!(v["output"]["properties"]["version"]["const"], 3);
    assert_eq!(
        v["output"]["properties"]["started_at"]["format"],
        "date-time"
    );
    assert_eq!(
        v["output"]["properties"]["submitted_at"]["format"],
        "date-time"
    );
}
