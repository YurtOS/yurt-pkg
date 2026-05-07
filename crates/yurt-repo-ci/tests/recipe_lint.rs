use std::fs;

use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn lint_accepts_v1_repo_signing_identity() {
    let dir = tempfile::tempdir().unwrap();
    let recipe = dir.path().join("recipe.toml");
    fs::write(
        &recipe,
        recipe_text(
            "https://github.com/YurtOS/yurt-packages/.github/workflows/release.yml@refs/heads/main",
        ),
    )
    .unwrap();

    let mut cmd = Command::cargo_bin("yurt-repo-ci").unwrap();
    cmd.args(["lint-recipe", recipe.to_str().unwrap()]);
    cmd.assert().success();
}

#[test]
fn lint_rejects_other_v1_signing_identity() {
    let dir = tempfile::tempdir().unwrap();
    let recipe = dir.path().join("recipe.toml");
    fs::write(&recipe, recipe_text("https://example.com/other")).unwrap();

    let mut cmd = Command::cargo_bin("yurt-repo-ci").unwrap();
    cmd.args(["lint-recipe", recipe.to_str().unwrap()]);
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("v1 signing subject"));
}

#[test]
fn lint_continuity_rejects_signer_change_without_migration() {
    let dir = tempfile::tempdir().unwrap();
    let package = dir.path().join("foo.json");
    fs::write(
        &package,
        r#"{
          "name": "foo",
          "versions": [{
            "version": "1.0.0",
            "build": "yurt_0",
            "url": "https://github.com/YurtOS/yurt-packages/releases/download/foo-1.0.0/foo-1.0.0.yurtpkg",
            "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "size": 1,
            "signing": {"subject": "old-subject", "issuer": "https://token.actions.githubusercontent.com"},
            "yanked": false
          }]
        }"#,
    )
    .unwrap();
    let recipe = dir.path().join("recipe.toml");
    fs::write(
        &recipe,
        recipe_text(
            "https://github.com/YurtOS/yurt-packages/.github/workflows/release.yml@refs/heads/main",
        ),
    )
    .unwrap();

    let mut cmd = Command::cargo_bin("yurt-repo-ci").unwrap();
    cmd.args([
        "lint-continuity",
        "--package-file",
        package.to_str().unwrap(),
        "--recipe",
        recipe.to_str().unwrap(),
    ]);
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("signer continuity"));
}

#[test]
fn lint_continuity_allows_matching_signer() {
    let dir = tempfile::tempdir().unwrap();
    let package = dir.path().join("foo.json");
    fs::write(
        &package,
        r#"{
          "name": "foo",
          "versions": [{
            "version": "1.0.0",
            "build": "yurt_0",
            "url": "https://github.com/YurtOS/yurt-packages/releases/download/foo-1.0.0/foo-1.0.0.yurtpkg",
            "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "size": 1,
            "signing": {
              "subject": "https://github.com/YurtOS/yurt-packages/.github/workflows/release.yml@refs/heads/main",
              "issuer": "https://token.actions.githubusercontent.com"
            },
            "yanked": false
          }]
        }"#,
    )
    .unwrap();
    let recipe = dir.path().join("recipe.toml");
    fs::write(
        &recipe,
        recipe_text(
            "https://github.com/YurtOS/yurt-packages/.github/workflows/release.yml@refs/heads/main",
        ),
    )
    .unwrap();

    let mut cmd = Command::cargo_bin("yurt-repo-ci").unwrap();
    cmd.args([
        "lint-continuity",
        "--package-file",
        package.to_str().unwrap(),
        "--recipe",
        recipe.to_str().unwrap(),
    ]);
    cmd.assert().success();
}

fn recipe_text(subject: &str) -> String {
    format!(
        r#"
[source]
url = "https://example.org/foo-1.0.0.tar.gz"
sha256 = "abc123"

[build]
steps = ["true"]
extended_build = false

[package]
name = "foo"
version = "1.0.0"
build = "yurt_0"
platform = "wasm32-wasip1-yurt"
summary = "Foo"
license = "MIT"
default_uid = 0
default_gid = 0

[package.depends]
libfoo = "^1.2"

[package.signing]
subject = "{subject}"
issuer = "https://token.actions.githubusercontent.com"
"#
    )
}
