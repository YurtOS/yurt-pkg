use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;
use yurt_pkg_format::validate_package_name;

#[derive(Debug, Error)]
pub enum Error {
    #[error("failed to parse trusted repos TOML: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("invalid repo id '{0}'")]
    InvalidRepoId(String),
    #[error("duplicate repo id '{0}'")]
    DuplicateRepoId(String),
    #[error("invalid repo url for '{id}': {source}")]
    InvalidRepoUrl {
        id: String,
        source: url::ParseError,
    },
    #[error("repo '{0}' has an empty signing subject")]
    EmptySubject(String),
    #[error("repo '{0}' has an empty signing issuer")]
    EmptyIssuer(String),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct SigningIdentity {
    pub subject: String,
    pub issuer: String,
}

impl SigningIdentity {
    pub fn matches(&self, subject: &str, issuer: &str) -> bool {
        self.subject == subject && self.issuer == issuer
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustRoot {
    pub fulcio_root_pem: PathBuf,
    pub rekor_public_key: PathBuf,
}

impl TrustRoot {
    pub fn from_dir(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        Self {
            fulcio_root_pem: dir.join("fulcio-root.pem"),
            rekor_public_key: dir.join("rekor.pub"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedRepo {
    pub id: String,
    pub url: Url,
    pub signing: SigningIdentity,
    pub priority: i64,
}

#[derive(Debug, Clone, Default)]
pub struct TrustedRepos {
    repos: BTreeMap<String, TrustedRepo>,
}

impl TrustedRepos {
    pub fn from_toml_str(text: &str) -> Result<Self> {
        let raw: RawTrustedRepos = toml::from_str(text)?;
        let mut seen = BTreeSet::new();
        let mut repos = BTreeMap::new();
        for repo in raw.repo {
            validate_package_name(&repo.id).map_err(|_| Error::InvalidRepoId(repo.id.clone()))?;
            if !seen.insert(repo.id.clone()) {
                return Err(Error::DuplicateRepoId(repo.id));
            }
            if repo.signing_subject.trim().is_empty() {
                return Err(Error::EmptySubject(repo.id));
            }
            if repo.signing_issuer.trim().is_empty() {
                return Err(Error::EmptyIssuer(repo.id));
            }
            let url = Url::parse(&repo.url).map_err(|source| Error::InvalidRepoUrl {
                id: repo.id.clone(),
                source,
            })?;
            let trusted = TrustedRepo {
                id: repo.id.clone(),
                url,
                signing: SigningIdentity {
                    subject: repo.signing_subject,
                    issuer: repo.signing_issuer,
                },
                priority: repo.priority.unwrap_or(0),
            };
            repos.insert(repo.id, trusted);
        }
        Ok(Self { repos })
    }

    pub fn get(&self, id: &str) -> Option<&TrustedRepo> {
        self.repos.get(id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &TrustedRepo> {
        self.repos.values()
    }
}

#[derive(Debug, Deserialize)]
struct RawTrustedRepos {
    #[serde(default)]
    repo: Vec<RawTrustedRepo>,
}

#[derive(Debug, Deserialize)]
struct RawTrustedRepo {
    id: String,
    url: String,
    signing_subject: String,
    signing_issuer: String,
    priority: Option<i64>,
}
