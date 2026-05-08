use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::{Duration, OffsetDateTime};
use url::Url;
use yurt_pkg_format::{validate_package_name, validate_sha256_hex, Depends};
use yurt_pkg_trust::SigningIdentity;

#[derive(Debug, Error)]
pub enum Error {
    #[error("unsupported schema {0}")]
    UnsupportedSchema(u32),
    #[error(
        "index rollback: new version {new_version} is not greater than cached {cached_version}"
    )]
    Rollback {
        new_version: u64,
        cached_version: u64,
    },
    #[error("index expired at {expires_at}")]
    Expired { expires_at: OffsetDateTime },
    #[error("invalid package name '{0}'")]
    InvalidPackageName(String),
    #[error("invalid package entry url for '{0}': {1}")]
    InvalidUrl(String, url::ParseError),
    #[error("invalid package entry relative url for '{package}': {url}")]
    InvalidPackageRelativeUrl { package: String, url: String },
    #[error("invalid sha256 for '{0}'")]
    InvalidSha256(String),
    #[error("package file name '{file_name}' does not match entry name '{version_name}'")]
    NameMismatch {
        file_name: String,
        version_name: String,
    },
    #[error("invalid dependency in '{package}': {message}")]
    InvalidDependency { package: String, message: String },
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy)]
pub struct Freshness {
    pub grace: Duration,
}

impl Default for Freshness {
    fn default() -> Self {
        Self {
            grace: Duration::days(30),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Index {
    pub schema: u32,
    pub index_version: u64,
    #[serde(with = "time::serde::rfc3339")]
    pub generated_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
    pub packages: BTreeMap<String, RepoPackage>,
}

impl Index {
    pub fn validate_against(
        &self,
        cached_version: Option<u64>,
        now: OffsetDateTime,
        freshness: Freshness,
    ) -> Result<()> {
        if self.schema != 1 {
            return Err(Error::UnsupportedSchema(self.schema));
        }
        if let Some(cached_version) = cached_version {
            if self.index_version <= cached_version {
                return Err(Error::Rollback {
                    new_version: self.index_version,
                    cached_version,
                });
            }
        }
        if now > self.expires_at + freshness.grace {
            return Err(Error::Expired {
                expires_at: self.expires_at,
            });
        }
        // `serde_json` keeps the last duplicate object key before we
        // see this map; repository CI must emit canonical JSON so
        // duplicate package entries never reach clients.
        for (name, package) in &self.packages {
            validate_package_name(name).map_err(|_| Error::InvalidPackageName(name.clone()))?;
            package.validate(name)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepoPackage {
    pub sha256: String,
    pub size: u64,
    pub url: String,
}

impl RepoPackage {
    fn validate(&self, name: &str) -> Result<()> {
        validate_sha256(name, &self.sha256)?;
        if Url::parse(&self.url).is_ok() {
            return Ok(());
        }
        if !self.url.starts_with('/') {
            validate_package_relative_url(name, &self.url)?;
        } else {
            return Err(Error::InvalidPackageRelativeUrl {
                package: name.to_string(),
                url: self.url.clone(),
            });
        }
        Ok(())
    }
}

fn validate_package_relative_url(name: &str, url: &str) -> Result<()> {
    if url.is_empty()
        || url.split('/').any(|part| part.is_empty() || part == "." || part == "..")
        || !url.ends_with(".json")
    {
        return Err(Error::InvalidPackageRelativeUrl {
            package: name.to_string(),
            url: url.to_string(),
        });
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackageFile {
    pub name: String,
    pub versions: Vec<PackageVersion>,
}

impl PackageFile {
    pub fn validate(&self) -> Result<()> {
        validate_package_name(&self.name)
            .map_err(|_| Error::InvalidPackageName(self.name.clone()))?;
        for version in &self.versions {
            if let Some(version_name) = &version.name {
                if version_name != &self.name {
                    return Err(Error::NameMismatch {
                        file_name: self.name.clone(),
                        version_name: version_name.clone(),
                    });
                }
            }
            version.validate(&self.name)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackageVersion {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub version: String,
    pub build: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
    pub signing: SigningIdentity,
    #[serde(default)]
    pub depends: Vec<Depends>,
    pub yanked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yanked_reason: Option<String>,
}

impl PackageVersion {
    fn validate(&self, package: &str) -> Result<()> {
        semver::Version::parse(&self.version).map_err(|err| Error::InvalidDependency {
            package: package.to_string(),
            message: format!("invalid package version '{}': {err}", self.version),
        })?;
        Url::parse(&self.url).map_err(|err| Error::InvalidUrl(package.to_string(), err))?;
        validate_sha256(package, &self.sha256)?;
        for dep in &self.depends {
            dep.validate().map_err(|err| Error::InvalidDependency {
                package: package.to_string(),
                message: err.to_string(),
            })?;
        }
        Ok(())
    }
}

fn validate_sha256(name: &str, value: &str) -> Result<()> {
    validate_sha256_hex(name, value).map_err(|_| Error::InvalidSha256(name.to_string()))
}
