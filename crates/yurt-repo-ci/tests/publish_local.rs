use std::fs;

use assert_cmd::Command;
use yurt_pkg_repo::metadata::{Index, PackageFile};

#[test]
fn publish_local_generates_repo_metadata_and_artifact_copy() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    let inputs = dir.path().join("inputs");
    fs::create_dir_all(&inputs).unwrap();

    let artifact = inputs.join("yurt-greet-0.1.0-yurt_0.yurtpkg");
    fs::write(&artifact, b"fake package bytes").unwrap();
    let manifest = inputs.join("yurt-pack.toml");
    fs::write(
        &manifest,
        r#"
name        = "yurt-greet"
version     = "0.1.0"
build       = "yurt_0"
platform    = "wasm32-wasip1-yurt"
summary     = "Greeting smoke package"
license     = "Apache-2.0"
default_uid = 0
default_gid = 0

[depends]
libc = "^0.1"
"#,
    )
    .unwrap();

    let mut cmd = Command::cargo_bin("yurt-repo-ci").unwrap();
    cmd.args([
        "publish-local",
        "--repo-root",
        repo.to_str().unwrap(),
        "--artifact",
        artifact.to_str().unwrap(),
        "--manifest",
        manifest.to_str().unwrap(),
        "--generated-at",
        "2026-05-09T00:00:00Z",
    ]);
    cmd.assert().success();

    let copied = repo
        .join("artifacts")
        .join("yurt-greet")
        .join("0.1.0")
        .join("yurt-greet-0.1.0-yurt_0.yurtpkg");
    assert_eq!(fs::read(copied).unwrap(), b"fake package bytes");
    assert!(repo.join("index.json.bundle").is_file());

    let index: Index = serde_json::from_slice(&fs::read(repo.join("index.json")).unwrap()).unwrap();
    assert_eq!(index.schema, 1);
    assert_eq!(index.index_version, 1);
    assert!(index.packages.contains_key("yurt-greet"));

    let package: PackageFile =
        serde_json::from_slice(&fs::read(repo.join("packages/yurt-greet.json")).unwrap()).unwrap();
    assert_eq!(package.name, "yurt-greet");
    assert_eq!(package.versions.len(), 1);
    let version = &package.versions[0];
    assert_eq!(version.version, "0.1.0");
    assert_eq!(version.build, "yurt_0");
    assert_eq!(version.depends.len(), 1);
    assert_eq!(version.depends[0].name, "libc");
    assert_eq!(
        version.url,
        "artifacts/yurt-greet/0.1.0/yurt-greet-0.1.0-yurt_0.yurtpkg"
    );
}
