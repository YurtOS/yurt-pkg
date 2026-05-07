use std::path::PathBuf;

use yurt_pkg_trust::{SigningIdentity, TrustRoot, TrustedRepos};

#[test]
fn parses_trusted_repos_with_subject_and_issuer() {
    let text = r#"
[[repo]]
id = "yurt-core"
url = "https://github.com/YurtOS/yurt-packages"
signing_subject = "https://github.com/YurtOS/yurt-packages/.github/workflows/release.yml@refs/heads/main"
signing_issuer = "https://token.actions.githubusercontent.com"
priority = 0
"#;

    let repos = TrustedRepos::from_toml_str(text).unwrap();
    let repo = repos.get("yurt-core").unwrap();
    assert_eq!(repo.priority, 0);
    assert_eq!(
        repo.signing,
        SigningIdentity {
            subject: "https://github.com/YurtOS/yurt-packages/.github/workflows/release.yml@refs/heads/main".to_string(),
            issuer: "https://token.actions.githubusercontent.com".to_string(),
        }
    );
}

#[test]
fn rejects_missing_issuer_and_duplicate_ids() {
    let missing_issuer = r#"
[[repo]]
id = "yurt-core"
url = "https://github.com/YurtOS/yurt-packages"
signing_subject = "subject"
priority = 0
"#;
    assert!(TrustedRepos::from_toml_str(missing_issuer).is_err());

    let duplicate = r#"
[[repo]]
id = "core"
url = "https://example.com/one"
signing_subject = "subject"
signing_issuer = "issuer"
priority = 0

[[repo]]
id = "core"
url = "https://example.com/two"
signing_subject = "subject"
signing_issuer = "issuer"
priority = 1
"#;
    assert!(TrustedRepos::from_toml_str(duplicate).is_err());
}

#[test]
fn signing_identity_matches_both_fields() {
    let expected = SigningIdentity {
        subject: "subject".to_string(),
        issuer: "issuer".to_string(),
    };
    assert!(expected.matches("subject", "issuer"));
    assert!(!expected.matches("subject", "other"));
    assert!(!expected.matches("other", "issuer"));
}

#[test]
fn trust_root_records_fulcio_and_rekor_paths() {
    let root = TrustRoot::from_dir(PathBuf::from("/etc/yurt-pkg/sigstore-trust-root"));
    assert_eq!(
        root.fulcio_root_pem,
        PathBuf::from("/etc/yurt-pkg/sigstore-trust-root/fulcio-root.pem")
    );
    assert_eq!(
        root.rekor_public_key,
        PathBuf::from("/etc/yurt-pkg/sigstore-trust-root/rekor.pub")
    );
}
