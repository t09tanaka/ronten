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
