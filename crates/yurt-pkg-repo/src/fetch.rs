//! Repository fetch boundary.

use thiserror::Error;
use url::Url;

#[cfg(any(test, feature = "test-fixtures"))]
use std::collections::BTreeMap;

#[derive(Debug, Error)]
pub enum Error {
    #[error("fetch url not found: {0}")]
    NotFound(Url),
    #[error("unsupported fetch scheme for {0}")]
    UnsupportedScheme(Url),
    #[error("failed to read local fetch path {path}: {source}")]
    Io {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy)]
pub struct FetchRequest<'a> {
    pub url: &'a Url,
    pub etag: Option<&'a str>,
    pub credential_origin: Option<&'a Url>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchResponse {
    NotModified,
    Modified { body: Vec<u8>, etag: Option<String> },
}

pub trait RepoFetcher {
    fn fetch(&self, request: FetchRequest<'_>) -> Result<FetchResponse>;
}

#[cfg(any(test, feature = "test-fixtures"))]
#[derive(Debug, Clone, Default)]
pub struct MemoryFetcher {
    entries: BTreeMap<Url, (Vec<u8>, Option<String>)>,
}

#[cfg(any(test, feature = "test-fixtures"))]
impl MemoryFetcher {
    pub fn insert(&mut self, url: Url, body: Vec<u8>, etag: Option<String>) {
        self.entries.insert(url, (body, etag));
    }
}

#[cfg(any(test, feature = "test-fixtures"))]
impl RepoFetcher for MemoryFetcher {
    fn fetch(&self, request: FetchRequest<'_>) -> Result<FetchResponse> {
        let Some((body, etag)) = self.entries.get(request.url) else {
            return Err(Error::NotFound(request.url.clone()));
        };
        if request.etag.is_some() && request.etag == etag.as_deref() {
            return Ok(FetchResponse::NotModified);
        }
        Ok(FetchResponse::Modified {
            body: body.clone(),
            etag: etag.clone(),
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct LocalFileFetcher;

impl RepoFetcher for LocalFileFetcher {
    fn fetch(&self, request: FetchRequest<'_>) -> Result<FetchResponse> {
        if request.url.scheme() != "file" {
            return Err(Error::UnsupportedScheme(request.url.clone()));
        }
        let path = request
            .url
            .to_file_path()
            .map_err(|_| Error::UnsupportedScheme(request.url.clone()))?;
        let body = std::fs::read(&path).map_err(|source| Error::Io { path, source })?;
        Ok(FetchResponse::Modified { body, etag: None })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_fetcher_returns_modified_and_etag() {
        let mut fetcher = MemoryFetcher::default();
        let url = Url::parse("file:///repo/index.json").unwrap();
        fetcher.insert(url.clone(), b"index".to_vec(), Some("etag-1".to_string()));

        let response = fetcher
            .fetch(FetchRequest {
                url: &url,
                etag: None,
                credential_origin: None,
            })
            .unwrap();

        assert_eq!(
            response,
            FetchResponse::Modified {
                body: b"index".to_vec(),
                etag: Some("etag-1".to_string()),
            }
        );
    }

    #[test]
    fn memory_fetcher_returns_not_modified_for_matching_etag() {
        let mut fetcher = MemoryFetcher::default();
        let url = Url::parse("file:///repo/index.json").unwrap();
        fetcher.insert(url.clone(), b"index".to_vec(), Some("etag-1".to_string()));

        let response = fetcher
            .fetch(FetchRequest {
                url: &url,
                etag: Some("etag-1"),
                credential_origin: None,
            })
            .unwrap();

        assert_eq!(response, FetchResponse::NotModified);
    }
}
