use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("missing required manifest: {0}")]
    MissingManifest(&'static str),

    #[error("invalid path '{path}': {reason}")]
    InvalidPath { path: String, reason: &'static str },

    #[error("duplicate entry after normalization: {0}")]
    DuplicateEntry(String),

    #[error("hash mismatch on '{path}': expected {expected}, got {actual}")]
    HashMismatch {
        path: String,
        expected: String,
        actual: String,
    },

    #[error("size mismatch on '{path}': expected {expected}, got {actual}")]
    SizeMismatch {
        path: String,
        expected: u64,
        actual: u64,
    },

    #[error("entry type mismatch on '{path}': manifest says {expected}, archive has {actual}")]
    EntryTypeMismatch {
        path: String,
        expected: &'static str,
        actual: &'static str,
    },

    #[error("manifest entry '{0}' has no matching archive entry")]
    ManifestEntryMissing(String),

    #[error("archive entry '{0}' has no matching manifest entry")]
    UnmanifestedEntry(String),

    #[error("unsupported tar entry type for '{path}': {kind}")]
    UnsupportedEntry { path: String, kind: String },

    #[error("invalid manifest: {0}")]
    InvalidManifest(String),
}

pub type Result<T> = std::result::Result<T, Error>;
