use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::process::Command;

use yurt_pkg_format::{FileEntryKind, Reader};

fn yurt_pack_bin() -> std::path::PathBuf {
    let mut p = std::path::PathBuf::from(env!("CARGO_BIN_EXE_yurt-pack"));
    assert!(p.exists(), "yurt-pack binary not found at {}", p.display());
    p.pop();
    p.push("yurt-pack");
    p
}

#[test]
fn builds_an_archive_from_a_staged_tree() {
    let temp = tempfile::tempdir().unwrap();

    // Stage a small tree mirroring the busybox shape from the spec.
    let stage = temp.path().join("stage");
    let bin = stage.join("bin");
    fs::create_dir_all(&bin).unwrap();
    fs::write(bin.join("demo"), b"#!fake\n").unwrap();
    fs::set_permissions(bin.join("demo"), fs::Permissions::from_mode(0o755)).unwrap();
    symlink("demo", bin.join("sh")).unwrap();

    // Manifest TOML with one declared hardlink.
    let manifest = temp.path().join("yurt-pack.toml");
    fs::write(
        &manifest,
        r#"
name        = "demo"
version     = "0.1.0"
build       = "yurt_0"
platform    = "wasm32-wasip1-yurt"
summary     = "Demo package"
license     = "Apache-2.0"
default_uid = 0
default_gid = 0

[depends]
libfoo = "^1.2"
libbar = ">=0.5, <1.0"

[yurt]
min_yurt_version = "0.1.0"
commands = ["demo", "sh"]

[yurt.requires]
processes = true

[[hardlinks]]
path   = "bin/demo2"
target = "bin/demo"
mode   = 0o755
"#,
    )
    .unwrap();

    let out = temp.path().join("out");
    let result = Command::new(yurt_pack_bin())
        .arg("build")
        .arg(&stage)
        .arg("--manifest")
        .arg(&manifest)
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "yurt-pack build exited with {}",
        result.status
    );
    // Canonical (0, 0) ownership: no non-canonical warning fires.
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(
        !stderr.contains("not a canonical"),
        "canonical ownership should not produce the non-canonical warning, got: {stderr}"
    );

    let artifact = out.join("demo-0.1.0-yurt_0.yurtpkg");
    assert!(artifact.exists(), "artifact not at {}", artifact.display());

    let f = fs::File::open(&artifact).unwrap();
    let r = Reader::read(f).unwrap();

    assert_eq!(r.index.name, "demo");
    assert_eq!(r.index.version, "0.1.0");
    assert_eq!(r.index.build, "yurt_0");
    assert_eq!(r.index.depends.len(), 2);
    assert_eq!(r.index.depends[0].name, "libbar");
    assert_eq!(r.index.depends[0].req, ">=0.5, <1.0");
    assert_eq!(r.index.depends[1].name, "libfoo");
    assert_eq!(r.index.depends[1].req, "^1.2");
    assert!(r.yurt.is_some());

    let by_path: std::collections::HashMap<&str, &yurt_pkg_format::FileEntry> =
        r.files.files.iter().map(|f| (f.path.as_str(), f)).collect();
    assert_eq!(by_path.get("bin/demo").unwrap().kind, FileEntryKind::File);
    assert_eq!(by_path.get("bin/sh").unwrap().kind, FileEntryKind::Symlink);
    assert_eq!(
        by_path.get("bin/demo2").unwrap().kind,
        FileEntryKind::Hardlink
    );
    assert_eq!(
        by_path.get("bin/demo2").unwrap().target.as_deref(),
        Some("bin/demo"),
    );
}

#[test]
fn rejects_manifest_without_default_uid() {
    let temp = tempfile::tempdir().unwrap();
    let stage = temp.path().join("stage");
    fs::create_dir_all(stage.join("bin")).unwrap();
    fs::write(stage.join("bin/demo"), b"x").unwrap();

    let manifest = temp.path().join("yurt-pack.toml");
    fs::write(
        &manifest,
        r#"
name        = "demo"
version     = "0.1.0"
build       = "yurt_0"
platform    = "wasm32-wasip1-yurt"
summary     = "Demo"
license     = "Apache-2.0"
"#,
    )
    .unwrap();

    let result = Command::new(yurt_pack_bin())
        .arg("build")
        .arg(&stage)
        .arg("--manifest")
        .arg(&manifest)
        .arg("--out")
        .arg(temp.path().join("out"))
        .output()
        .unwrap();
    assert!(
        !result.status.success(),
        "should have refused to assume an ownership default"
    );
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("default_uid"), "stderr was: {stderr}");
}

#[test]
fn warns_on_non_canonical_ownership() {
    let temp = tempfile::tempdir().unwrap();
    let stage = temp.path().join("stage");
    fs::create_dir_all(stage.join("bin")).unwrap();
    fs::write(stage.join("bin/demo"), b"x").unwrap();

    let manifest = temp.path().join("yurt-pack.toml");
    fs::write(
        &manifest,
        r#"
name        = "demo"
version     = "0.1.0"
build       = "yurt_0"
platform    = "wasm32-wasip1-yurt"
summary     = "Demo"
license     = "Apache-2.0"
default_uid = 500
default_gid = 500
"#,
    )
    .unwrap();

    let result = Command::new(yurt_pack_bin())
        .arg("build")
        .arg(&stage)
        .arg("--manifest")
        .arg(&manifest)
        .arg("--out")
        .arg(temp.path().join("out"))
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "non-canonical ownership should warn, not fail"
    );
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("not a canonical"), "stderr was: {stderr}");
}

#[test]
fn rejects_traversal_in_hardlink_target() {
    let temp = tempfile::tempdir().unwrap();
    let stage = temp.path().join("stage");
    fs::create_dir_all(stage.join("bin")).unwrap();
    fs::write(stage.join("bin/demo"), b"x").unwrap();

    let manifest = temp.path().join("yurt-pack.toml");
    fs::write(
        &manifest,
        r#"
name        = "demo"
version     = "0.1.0"
build       = "yurt_0"
platform    = "wasm32-wasip1-yurt"
summary     = "Demo"
license     = "Apache-2.0"
default_uid = 0
default_gid = 0

[[hardlinks]]
path   = "bin/escape"
target = "../../etc/passwd"
"#,
    )
    .unwrap();

    let out = temp.path().join("out");
    let result = Command::new(yurt_pack_bin())
        .arg("build")
        .arg(&stage)
        .arg("--manifest")
        .arg(&manifest)
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();
    assert!(!result.status.success(), "should have rejected traversal");
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("hardlink target"), "stderr was: {}", stderr);
}
