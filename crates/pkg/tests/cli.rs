use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn top_level_help_lists_stubbed_command_surface() {
    let mut cmd = Command::cargo_bin("pkg").unwrap();
    cmd.arg("--help");
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("update"))
        .stdout(predicate::str::contains("add-repo"))
        .stdout(predicate::str::contains("install"));
}

#[test]
fn add_repo_help_lists_required_signing_flags() {
    let mut cmd = Command::cargo_bin("pkg").unwrap();
    cmd.args(["add-repo", "--help"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("--signing-subject"))
        .stdout(predicate::str::contains("--signing-issuer"));
}

#[test]
fn add_repo_requires_subject_and_issuer() {
    let mut cmd = Command::cargo_bin("pkg").unwrap();
    cmd.args([
        "add-repo",
        "https://example.com/repo",
        "--signing-subject",
        "subject",
    ]);
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("--signing-issuer"));
}

#[test]
fn install_reports_planner_boundary() {
    let mut cmd = Command::cargo_bin("pkg").unwrap();
    cmd.args(["install", "foo"]);
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("resolver/installer spec"));
}
