use thiserror::Error;
use time::OffsetDateTime;
use yurt_pkg_trust::{SigningIdentity, TrustRoot};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationInput<'a> {
    pub payload: &'a [u8],
    pub bundle: &'a [u8],
    pub expected_signing: &'a SigningIdentity,
    pub trust_root: &'a TrustRoot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationOutput {
    pub integrated_time: OffsetDateTime,
    pub subject: String,
    pub issuer: String,
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("signing identity mismatch: expected {expected_subject} / {expected_issuer}, got {actual_subject} / {actual_issuer}")]
    SigningIdentityMismatch {
        expected_subject: String,
        expected_issuer: String,
        actual_subject: String,
        actual_issuer: String,
    },
    #[error("bundle verification is not wired to sigstore yet")]
    NotImplemented,
}

pub type Result<T> = std::result::Result<T, Error>;

pub trait BundleVerifier {
    fn verify(&self, input: VerificationInput<'_>) -> Result<VerificationOutput>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NotImplementedVerifier;

impl BundleVerifier for NotImplementedVerifier {
    fn verify(&self, _input: VerificationInput<'_>) -> Result<VerificationOutput> {
        Err(Error::NotImplemented)
    }
}

#[cfg(any(test, feature = "test-fixtures"))]
#[derive(Debug, Clone)]
pub struct StaticVerifier {
    pub output: VerificationOutput,
}

#[cfg(any(test, feature = "test-fixtures"))]
impl BundleVerifier for StaticVerifier {
    fn verify(&self, input: VerificationInput<'_>) -> Result<VerificationOutput> {
        if !input
            .expected_signing
            .matches(&self.output.subject, &self.output.issuer)
        {
            return Err(Error::SigningIdentityMismatch {
                expected_subject: input.expected_signing.subject.clone(),
                expected_issuer: input.expected_signing.issuer.clone(),
                actual_subject: self.output.subject.clone(),
                actual_issuer: self.output.issuer.clone(),
            });
        }
        Ok(self.output.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_verifier_checks_subject_and_issuer() {
        let verifier = StaticVerifier {
            output: VerificationOutput {
                integrated_time: OffsetDateTime::UNIX_EPOCH,
                subject: "subject".to_string(),
                issuer: "issuer".to_string(),
            },
        };
        let expected = SigningIdentity {
            subject: "subject".to_string(),
            issuer: "issuer".to_string(),
        };
        let trust_root = TrustRoot::from_dir("/etc/yurt-pkg/sigstore-trust-root");
        verifier
            .verify(VerificationInput {
                payload: b"index",
                bundle: b"bundle",
                expected_signing: &expected,
                trust_root: &trust_root,
            })
            .unwrap();

        let wrong = SigningIdentity {
            subject: "subject".to_string(),
            issuer: "other".to_string(),
        };
        assert!(verifier
            .verify(VerificationInput {
                payload: b"index",
                bundle: b"bundle",
                expected_signing: &wrong,
                trust_root: &trust_root,
            })
            .is_err());
    }
}
