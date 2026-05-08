use assert_cmd::Command;
use predicates::prelude::*;
use sha2::{Digest, Sha256};
use std::fs;
use tempfile::{tempdir, TempDir};
use time::macros::datetime;
use yurt_pkg_repo::metadata::{Index, PackageFile};
use yurt_pkg_repo::search_index::RepoSearchIndex;
use yurt_pkg_repo::state::{RepoState, SnapshotManifest};
use yurt_pkg_repo::store::RepoCacheStore;

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

#[cfg(feature = "test-fixtures")]
#[test]
fn cli_update_populates_cache_from_file_repo() {
    let fixture = RepoFixture::new();
    let mut cmd = feature_pkg_cmd();
    cmd.args([
        "--etc-root",
        fixture.etc.path().to_str().unwrap(),
        "--cache-root",
        fixture.cache.path().to_str().unwrap(),
        "update",
    ]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("updated official"));
}

#[cfg(feature = "test-fixtures")]
#[test]
fn cli_update_reports_consecutive_failure_count() {
    let fixture = RepoFixture::new();
    fs::remove_file(fixture.repo.path().join("index.json")).unwrap();
    for _ in 0..2 {
        let mut cmd = feature_pkg_cmd();
        cmd.args([
            "--etc-root",
            fixture.etc.path().to_str().unwrap(),
            "--cache-root",
            fixture.cache.path().to_str().unwrap(),
            "update",
        ]);
        cmd.assert().failure();
    }
    let mut cmd = feature_pkg_cmd();
    cmd.args([
        "--etc-root",
        fixture.etc.path().to_str().unwrap(),
        "--cache-root",
        fixture.cache.path().to_str().unwrap(),
        "update",
    ]);
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("3 consecutive update failures"));
}

#[test]
fn cli_search_reads_cache_without_network() {
    let fixture = RepoFixture::new();
    fixture.populate_cache();
    let mut cmd = Command::cargo_bin("pkg").unwrap();
    cmd.args([
        "--etc-root",
        fixture.etc.path().to_str().unwrap(),
        "--cache-root",
        fixture.cache.path().to_str().unwrap(),
        "search",
        "tool",
    ]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("tool"))
        .stdout(predicate::str::contains("official"));
}

#[test]
fn cli_info_lists_versions_and_dependencies() {
    let fixture = RepoFixture::new();
    fixture.populate_cache();
    let mut cmd = Command::cargo_bin("pkg").unwrap();
    cmd.args([
        "--etc-root",
        fixture.etc.path().to_str().unwrap(),
        "--cache-root",
        fixture.cache.path().to_str().unwrap(),
        "info",
        "tool",
    ]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("version: 1.0.0-yurt_0"))
        .stdout(predicate::str::contains("signing: subject / issuer"))
        .stdout(predicate::str::contains("libc ^0.1"));
}

#[test]
fn cli_info_warns_on_url_only_change() {
    let fixture = RepoFixture::new();
    fixture.populate_cache();
    fixture.write_trusted("file:///tmp/other-repo/", "subject", "issuer");
    let mut cmd = Command::cargo_bin("pkg").unwrap();
    cmd.args([
        "--etc-root",
        fixture.etc.path().to_str().unwrap(),
        "--cache-root",
        fixture.cache.path().to_str().unwrap(),
        "info",
        "tool",
    ]);
    cmd.assert()
        .success()
        .stderr(predicate::str::contains("URL changed"))
        .stdout(predicate::str::contains("tool"));
}

#[test]
fn cli_info_refuses_signing_identity_change() {
    let fixture = RepoFixture::new();
    fixture.populate_cache();
    fixture.write_trusted(
        url::Url::from_directory_path(fixture.repo.path())
            .unwrap()
            .as_ref(),
        "other-subject",
        "issuer",
    );
    let mut cmd = Command::cargo_bin("pkg").unwrap();
    cmd.args([
        "--etc-root",
        fixture.etc.path().to_str().unwrap(),
        "--cache-root",
        fixture.cache.path().to_str().unwrap(),
        "info",
        "tool",
    ]);
    cmd.assert().failure().stderr(predicate::str::contains(
        "trusted config for repo official changed",
    ));
}

#[test]
fn cli_search_skips_repo_with_signing_identity_change_and_uses_other_repos() {
    let fixture = RepoFixture::new();
    fixture.populate_cache();
    fixture.populate_cache_for("overlay", "subject", "issuer");
    let repo_url = url::Url::from_directory_path(fixture.repo.path())
        .unwrap()
        .to_string();
    write_trusted_entries(
        &fixture.etc,
        &[
            ("official", &repo_url, "other-subject", "issuer", 10),
            ("overlay", &repo_url, "subject", "issuer", 0),
        ],
    );
    let mut cmd = Command::cargo_bin("pkg").unwrap();
    cmd.args([
        "--etc-root",
        fixture.etc.path().to_str().unwrap(),
        "--cache-root",
        fixture.cache.path().to_str().unwrap(),
        "search",
        "tool",
    ]);
    cmd.assert()
        .success()
        .stderr(predicate::str::contains(
            "trusted config for repo official changed",
        ))
        .stdout(predicate::str::contains("tool"))
        .stdout(predicate::str::contains("overlay"))
        .stdout(predicate::str::contains("official").not());
}

#[test]
fn cli_info_unknown_package_exits_nonzero_and_suggests_update() {
    let fixture = RepoFixture::new();
    fixture.populate_cache();
    let mut cmd = Command::cargo_bin("pkg").unwrap();
    cmd.args([
        "--etc-root",
        fixture.etc.path().to_str().unwrap(),
        "--cache-root",
        fixture.cache.path().to_str().unwrap(),
        "info",
        "missing",
    ]);
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("not found in local cache"))
        .stderr(predicate::str::contains("run pkg update"));
}

#[test]
fn cli_info_unknown_repo_filter_exits_nonzero_and_suggests_update() {
    let fixture = RepoFixture::new();
    fixture.populate_cache();
    let mut cmd = Command::cargo_bin("pkg").unwrap();
    cmd.args([
        "--etc-root",
        fixture.etc.path().to_str().unwrap(),
        "--cache-root",
        fixture.cache.path().to_str().unwrap(),
        "info",
        "tool",
        "--repo",
        "typo",
    ]);
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("not found in local cache"))
        .stderr(predicate::str::contains("run pkg update"));
}

#[test]
fn cli_search_warns_on_stale_cache_and_failure_count() {
    let fixture = RepoFixture::new();
    fixture.populate_cache();
    fixture.make_cache_stale_with_failures();
    let mut cmd = Command::cargo_bin("pkg").unwrap();
    cmd.args([
        "--etc-root",
        fixture.etc.path().to_str().unwrap(),
        "--cache-root",
        fixture.cache.path().to_str().unwrap(),
        "search",
        "tool",
    ]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("tool"))
        .stderr(predicate::str::contains("cache is stale"))
        .stderr(predicate::str::contains("2 consecutive update failures"));
}

#[test]
fn cli_search_no_cache_exits_nonzero_and_suggests_update() {
    let fixture = RepoFixture::new();
    let mut cmd = Command::cargo_bin("pkg").unwrap();
    cmd.args([
        "--etc-root",
        fixture.etc.path().to_str().unwrap(),
        "--cache-root",
        fixture.cache.path().to_str().unwrap(),
        "search",
        "tool",
    ]);
    cmd.assert()
        .failure()
        .stderr(predicate::str::contains("run pkg update"));
}

struct RepoFixture {
    etc: TempDir,
    cache: TempDir,
    repo: TempDir,
}

impl RepoFixture {
    fn new() -> Self {
        let etc = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let repo = tempdir().unwrap();
        fs::create_dir_all(etc.path().join("yurt-pkg/sigstore-trust-root")).unwrap();
        write_trusted_file(
            &etc,
            url::Url::from_directory_path(repo.path()).unwrap().as_ref(),
            "subject",
            "issuer",
        );
        fs::create_dir_all(repo.path().join("packages")).unwrap();
        let package = package_json();
        fs::write(repo.path().join("packages/tool.json"), package.as_bytes()).unwrap();
        let hash = hex(&Sha256::digest(package.as_bytes()));
        fs::write(
            repo.path().join("index.json"),
            format!(
                r#"{{
  "schema": 1,
  "index_version": 1,
  "generated_at": "2026-05-08T00:00:00Z",
  "expires_at": "2099-01-01T00:00:00Z",
  "packages": {{
    "tool": {{"sha256": "{hash}", "size": {}, "url": "packages/tool.json"}}
  }}
}}"#,
                package.len()
            ),
        )
        .unwrap();
        fs::write(repo.path().join("index.json.bundle"), b"bundle").unwrap();
        Self { etc, cache, repo }
    }

    fn write_trusted(&self, url: &str, subject: &str, issuer: &str) {
        write_trusted_file(&self.etc, url, subject, issuer);
    }

    fn make_cache_stale_with_failures(&self) {
        let store = RepoCacheStore::new(self.cache.path());
        let snapshot = store.current_snapshot_id("official").unwrap().unwrap();
        let snapshot_dir = store.snapshot_dir("official", &snapshot);
        let mut manifest: SnapshotManifest =
            serde_json::from_slice(&fs::read(snapshot_dir.join("manifest.json")).unwrap()).unwrap();
        manifest.expires_at = datetime!(2000-01-01 00:00 UTC);
        fs::write(
            snapshot_dir.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        let mut state = store.read_state("official").unwrap().unwrap();
        state.consecutive_fetch_failures = 2;
        store.write_state("official", &state).unwrap();
    }

    fn populate_cache(&self) {
        self.populate_cache_for("official", "subject", "issuer");
    }

    fn populate_cache_for(&self, repo_id: &str, signing_subject: &str, signing_issuer: &str) {
        let store = RepoCacheStore::new(self.cache.path());
        let staging = store.staging_dir(repo_id, "fixture");
        fs::create_dir_all(staging.join("packages")).unwrap();
        fs::copy(
            self.repo.path().join("index.json"),
            staging.join("index.json"),
        )
        .unwrap();
        fs::copy(
            self.repo.path().join("index.json.bundle"),
            staging.join("index.json.bundle"),
        )
        .unwrap();
        fs::copy(
            self.repo.path().join("packages/tool.json"),
            staging.join("packages/tool.json"),
        )
        .unwrap();
        let index: Index =
            serde_json::from_slice(&fs::read(staging.join("index.json")).unwrap()).unwrap();
        let package: PackageFile =
            serde_json::from_slice(&fs::read(staging.join("packages/tool.json")).unwrap()).unwrap();
        let manifest = SnapshotManifest {
            schema: 1,
            repo_id: repo_id.to_string(),
            repo_url: url::Url::from_directory_path(self.repo.path())
                .unwrap()
                .to_string(),
            signing_subject: signing_subject.to_string(),
            signing_issuer: signing_issuer.to_string(),
            index_version: index.index_version,
            integrated_time: datetime!(2026-05-08 00:00 UTC),
            expires_at: index.expires_at,
        };
        fs::write(
            staging.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        RepoSearchIndex::rebuild(staging.join("db.sqlite"), repo_id, &[package]).unwrap();
        store
            .commit_staging(repo_id, &staging, "fixture-snapshot")
            .unwrap();
        store
            .write_state(
                repo_id,
                &RepoState {
                    schema: 1,
                    repo_id: repo_id.to_string(),
                    current_snapshot: "fixture-snapshot".to_string(),
                    index_etag: None,
                    index_bundle_etag: None,
                    last_fetched: datetime!(2026-05-08 00:00 UTC),
                    consecutive_fetch_failures: 0,
                },
            )
            .unwrap();
    }
}

fn write_trusted_file(etc: &TempDir, url: &str, subject: &str, issuer: &str) {
    write_trusted_entries(etc, &[("official", url, subject, issuer, 0)]);
}

fn write_trusted_entries(etc: &TempDir, repos: &[(&str, &str, &str, &str, i64)]) {
    let mut text = String::new();
    for (id, url, subject, issuer, priority) in repos {
        text.push_str(&format!(
            r#"
[[repo]]
id = "{id}"
url = "{url}"
signing_subject = "{subject}"
signing_issuer = "{issuer}"
priority = {priority}
"#,
        ));
    }
    fs::write(etc.path().join("yurt-pkg/trusted-repos.toml"), text).unwrap();
}

fn package_json() -> String {
    r#"{
  "name": "tool",
  "versions": [{
    "version": "1.0.0",
    "build": "yurt_0",
    "url": "https://example.com/tool.yurtpkg",
    "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    "size": 1,
    "signing": {"subject": "subject", "issuer": "issuer"},
    "depends": [{"name": "libc", "req": "^0.1"}],
    "yanked": false
  }]
}"#
    .to_string()
}

fn hex(bytes: &[u8]) -> String {
    const CHARS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(CHARS[(byte >> 4) as usize] as char);
        out.push(CHARS[(byte & 0xf) as usize] as char);
    }
    out
}

#[cfg(feature = "test-fixtures")]
fn feature_pkg_cmd() -> Command {
    let mut cmd = Command::new(env!("CARGO"));
    cmd.current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."));
    cmd.args(["run", "-p", "pkg", "--features", "test-fixtures", "--"]);
    cmd
}
