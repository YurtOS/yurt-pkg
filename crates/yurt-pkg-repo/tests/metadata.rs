use time::OffsetDateTime;
use yurt_pkg_repo::metadata::{Freshness, Index, PackageFile};

#[test]
fn index_rejects_rollback_and_expired_metadata() {
    let json = r#"{
      "schema": 1,
      "index_version": 10,
      "generated_at": "2026-05-07T12:00:00Z",
      "expires_at": "2026-05-14T12:00:00Z",
      "packages": {
        "foo": {"sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "size": 123, "url": "packages/foo.json"}
      }
    }"#;
    let index: Index = serde_json::from_str(json).unwrap();
    let now = OffsetDateTime::parse(
        "2026-05-08T00:00:00Z",
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap();
    index
        .validate_against(Some(9), now, Freshness::default())
        .unwrap();
    assert!(index
        .validate_against(Some(10), now, Freshness::default())
        .is_err());

    let late = OffsetDateTime::parse(
        "2026-06-20T00:00:00Z",
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap();
    assert!(index
        .validate_against(Some(9), late, Freshness::default())
        .is_err());
}

#[test]
fn index_accepts_package_json_relative_urls() {
    let json = r#"{
      "schema": 1,
      "index_version": 10,
      "generated_at": "2026-05-07T12:00:00Z",
      "expires_at": "2026-05-14T12:00:00Z",
      "packages": {
        "foo": {"sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "size": 123, "url": "packages/foo.json"}
      }
    }"#;
    let index: Index = serde_json::from_str(json).unwrap();
    let now = OffsetDateTime::parse(
        "2026-05-08T00:00:00Z",
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap();
    index
        .validate_against(None, now, Freshness::default())
        .unwrap();
}

#[test]
fn index_rejects_unsafe_package_relative_urls() {
    for url in [
        "/packages/foo.json",
        "packages/../foo.json",
        "packages/foo.txt",
        "packages//foo.json",
    ] {
        let json = format!(
            r#"{{
              "schema": 1,
              "index_version": 10,
              "generated_at": "2026-05-07T12:00:00Z",
              "expires_at": "2026-05-14T12:00:00Z",
              "packages": {{
                "foo": {{"sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "size": 123, "url": "{url}"}}
              }}
            }}"#
        );
        let index: Index = serde_json::from_str(&json).unwrap();
        let now = OffsetDateTime::parse(
            "2026-05-08T00:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap();
        assert!(
            index
                .validate_against(None, now, Freshness::default())
                .is_err(),
            "expected {url} to be rejected"
        );
    }
}

#[test]
fn index_accepts_safe_noncanonical_relative_package_urls() {
    let json = r#"{
      "schema": 1,
      "index_version": 10,
      "generated_at": "2026-05-07T12:00:00Z",
      "expires_at": "2026-05-14T12:00:00Z",
      "packages": {
        "foo": {"sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "size": 123, "url": "pkg/foo-v1.json"}
      }
    }"#;
    let index: Index = serde_json::from_str(json).unwrap();
    let now = OffsetDateTime::parse(
        "2026-05-08T00:00:00Z",
        &time::format_description::well_known::Rfc3339,
    )
    .unwrap();
    index
        .validate_against(None, now, Freshness::default())
        .unwrap();
}

#[test]
fn package_file_validates_signing_and_dependencies() {
    let json = r#"{
      "name": "foo",
      "versions": [{
        "version": "1.0.0",
        "build": "yurt_0",
        "url": "https://github.com/YurtOS/yurt-packages/releases/download/foo-1.0.0/foo-1.0.0.yurtpkg",
        "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "size": 56789,
        "signing": {
          "subject": "https://github.com/YurtOS/yurt-packages/.github/workflows/release.yml@refs/heads/main",
          "issuer": "https://token.actions.githubusercontent.com"
        },
        "depends": [{"name": "libfoo", "req": "^1.2"}],
        "yanked": false
      }]
    }"#;
    let package: PackageFile = serde_json::from_str(json).unwrap();
    package.validate().unwrap();
    assert_eq!(package.versions[0].depends[0].name, "libfoo");
}
