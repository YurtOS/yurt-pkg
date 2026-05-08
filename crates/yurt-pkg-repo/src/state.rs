//! Repository snapshot state.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use yurt_pkg_trust::TrustedRepo;

use crate::metadata::Index;
use crate::verify::VerificationOutput;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotManifest {
    pub schema: u32,
    pub repo_id: String,
    pub repo_url: String,
    pub signing_subject: String,
    pub signing_issuer: String,
    pub index_version: u64,
    #[serde(with = "time::serde::rfc3339")]
    pub integrated_time: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepoState {
    pub schema: u32,
    pub repo_id: String,
    pub current_snapshot: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_etag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_bundle_etag: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub last_fetched: OffsetDateTime,
    pub consecutive_fetch_failures: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustChange {
    Unchanged,
    UrlOnly,
    SigningIdentity,
}

impl SnapshotManifest {
    pub fn from_verified_index(
        repo: &TrustedRepo,
        index: &Index,
        verification: &VerificationOutput,
    ) -> Self {
        Self {
            schema: 1,
            repo_id: repo.id.clone(),
            repo_url: repo.url.to_string(),
            signing_subject: verification.subject.clone(),
            signing_issuer: verification.issuer.clone(),
            index_version: index.index_version,
            integrated_time: verification.integrated_time,
            expires_at: index.expires_at,
        }
    }

    pub fn trust_change(&self, repo: &TrustedRepo) -> TrustChange {
        if self.signing_subject != repo.signing.subject
            || self.signing_issuer != repo.signing.issuer
        {
            return TrustChange::SigningIdentity;
        }
        if self.repo_url != repo.url.to_string() {
            return TrustChange::UrlOnly;
        }
        TrustChange::Unchanged
    }
}

impl RepoState {
    pub fn without_etags_for_repair(
        repo_id: String,
        current_snapshot: String,
        now: OffsetDateTime,
    ) -> Self {
        Self {
            schema: 1,
            repo_id,
            current_snapshot,
            index_etag: None,
            index_bundle_etag: None,
            last_fetched: now,
            consecutive_fetch_failures: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use time::macros::datetime;
    use url::Url;
    use yurt_pkg_trust::{SigningIdentity, TrustedRepo};

    use super::*;

    fn trusted() -> TrustedRepo {
        TrustedRepo {
            id: "official".to_string(),
            url: Url::parse("https://example.com/repo").unwrap(),
            signing: SigningIdentity {
                subject: "subject".into(),
                issuer: "issuer".into(),
            },
            priority: 0,
        }
    }

    fn index() -> Index {
        Index {
            schema: 1,
            index_version: 7,
            generated_at: datetime!(2026-05-07 12:00 UTC),
            expires_at: datetime!(2026-05-14 12:00 UTC),
            packages: BTreeMap::new(),
        }
    }

    fn verification(subject: &str, issuer: &str) -> VerificationOutput {
        VerificationOutput {
            integrated_time: datetime!(2026-05-07 12:01 UTC),
            subject: subject.to_string(),
            issuer: issuer.to_string(),
        }
    }

    #[test]
    fn manifest_serializes_trust_binding_without_priority() {
        let manifest = SnapshotManifest::from_verified_index(
            &trusted(),
            &index(),
            &verification("subject", "issuer"),
        );
        let json = serde_json::to_string(&manifest).unwrap();
        assert!(json.contains("subject"));
        assert!(json.contains("issuer"));
        assert!(json.contains("https://example.com/repo"));
        assert!(!json.contains("priority"));
    }

    #[test]
    fn signing_identity_change_resets_security_state() {
        let manifest = SnapshotManifest::from_verified_index(
            &trusted(),
            &index(),
            &verification("actual-subject", "issuer"),
        );
        assert_eq!(
            manifest.trust_change(&trusted()),
            TrustChange::SigningIdentity
        );
    }

    #[test]
    fn url_only_change_keeps_security_state_but_suppresses_fetch_reuse() {
        let manifest = SnapshotManifest::from_verified_index(
            &trusted(),
            &index(),
            &verification("subject", "issuer"),
        );
        let mut changed = trusted();
        changed.url = Url::parse("https://mirror.example/repo").unwrap();
        assert_eq!(manifest.trust_change(&changed), TrustChange::UrlOnly);
    }

    #[test]
    fn priority_only_change_is_not_a_cache_binding_change() {
        let manifest = SnapshotManifest::from_verified_index(
            &trusted(),
            &index(),
            &verification("subject", "issuer"),
        );
        let mut changed = trusted();
        changed.priority = -10;
        assert_eq!(manifest.trust_change(&changed), TrustChange::Unchanged);
    }
}
