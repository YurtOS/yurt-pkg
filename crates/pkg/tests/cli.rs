use assert_cmd::Command;
use predicates::prelude::*;

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
