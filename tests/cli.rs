use assert_cmd::Command;

#[test]
fn help_lists_subcommands() {
    let out = Command::cargo_bin("ronten")
        .unwrap()
        .arg("--help")
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    for sub in ["review", "schema", "demo"] {
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
