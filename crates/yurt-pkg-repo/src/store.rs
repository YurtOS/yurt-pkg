//! Repository cache store.

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use fs2::FileExt;
use sha2::{Digest, Sha256};
use thiserror::Error;
use time::OffsetDateTime;

use crate::state::{RepoState, SnapshotManifest};

static SNAPSHOT_COUNTER: AtomicU64 = AtomicU64::new(0);
static STATE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Error)]
pub enum Error {
    #[error("repo cache io error at {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("repo cache json error at {path}: {source}")]
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("current snapshot link for repo '{0}' points outside snapshots/")]
    InvalidCurrentLink(String),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone)]
pub struct RepoCacheStore {
    root: PathBuf,
}

#[derive(Debug, Clone, Copy)]
pub enum LockMode {
    Shared,
    Exclusive,
}

pub struct RepoLock {
    file: File,
}

impl Drop for RepoLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

impl RepoCacheStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn repo_dir(&self, repo_id: &str) -> PathBuf {
        self.root.join(repo_id)
    }

    pub fn snapshot_dir(&self, repo_id: &str, snapshot_id: &str) -> PathBuf {
        self.repo_dir(repo_id).join("snapshots").join(snapshot_id)
    }

    pub fn staging_dir(&self, repo_id: &str, suffix: &str) -> PathBuf {
        self.repo_dir(repo_id).join(format!("staging-{suffix}"))
    }

    pub fn lock(&self, repo_id: &str, mode: LockMode) -> Result<RepoLock> {
        let repo_dir = self.repo_dir(repo_id);
        fs::create_dir_all(&repo_dir).map_err(|source| Error::Io {
            path: repo_dir.clone(),
            source,
        })?;
        let lock_path = repo_dir.join(".lock");
        let file = File::options()
            .create(true)
            .read(true)
            .write(true)
            .open(&lock_path)
            .map_err(|source| Error::Io {
                path: lock_path.clone(),
                source,
            })?;
        match mode {
            LockMode::Shared => file.lock_shared(),
            LockMode::Exclusive => file.lock_exclusive(),
        }
        .map_err(|source| Error::Io {
            path: lock_path,
            source,
        })?;
        Ok(RepoLock { file })
    }

    pub fn current_snapshot_id(&self, repo_id: &str) -> Result<Option<String>> {
        let current = self.repo_dir(repo_id).join("current");
        match fs::read_link(&current) {
            Ok(link) => {
                let mut components = link.components();
                let first = components.next().and_then(|c| c.as_os_str().to_str());
                let second = components.next().and_then(|c| c.as_os_str().to_str());
                if first == Some("snapshots") && components.next().is_none() {
                    return Ok(second.map(ToOwned::to_owned));
                }
                Err(Error::InvalidCurrentLink(repo_id.to_string()))
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(Error::Io {
                path: current,
                source,
            }),
        }
    }

    pub fn read_current_manifest(&self, repo_id: &str) -> Result<Option<SnapshotManifest>> {
        let Some(snapshot_id) = self.current_snapshot_id(repo_id)? else {
            return Ok(None);
        };
        let path = self
            .snapshot_dir(repo_id, &snapshot_id)
            .join("manifest.json");
        read_json(&path).map(Some)
    }

    pub fn read_state(&self, repo_id: &str) -> Result<Option<RepoState>> {
        let path = self.repo_dir(repo_id).join("state.json");
        if !path.exists() {
            return Ok(None);
        }
        read_json(&path).map(Some)
    }

    pub fn write_state(&self, repo_id: &str, state: &RepoState) -> Result<()> {
        let repo_dir = self.repo_dir(repo_id);
        fs::create_dir_all(&repo_dir).map_err(|source| Error::Io {
            path: repo_dir.clone(),
            source,
        })?;
        let suffix = format!(
            "{}-{}-{}",
            state.current_snapshot,
            std::process::id(),
            STATE_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let tmp_path = repo_dir.join(format!("state.json.tmp-{suffix}"));
        let path = repo_dir.join("state.json");
        let bytes = serde_json::to_vec_pretty(state).map_err(|source| Error::Json {
            path: path.clone(),
            source,
        })?;
        fs::write(&tmp_path, bytes).map_err(|source| Error::Io {
            path: tmp_path.clone(),
            source,
        })?;
        fs::rename(&tmp_path, &path).map_err(|source| Error::Io { path, source })?;
        Ok(())
    }

    pub fn repair_state_if_needed(
        &self,
        repo_id: &str,
        now: OffsetDateTime,
    ) -> Result<Option<RepoState>> {
        let Some(current_snapshot) = self.current_snapshot_id(repo_id)? else {
            return Ok(None);
        };
        let state = self.read_state(repo_id)?;
        if state
            .as_ref()
            .is_some_and(|state| state.current_snapshot == current_snapshot)
        {
            return Ok(state);
        }
        let repaired =
            RepoState::without_etags_for_repair(repo_id.to_string(), current_snapshot, now);
        self.write_state(repo_id, &repaired)?;
        Ok(Some(repaired))
    }

    pub fn commit_staging(&self, repo_id: &str, staging: &Path, snapshot_id: &str) -> Result<()> {
        let repo_dir = self.repo_dir(repo_id);
        let snapshots = repo_dir.join("snapshots");
        fs::create_dir_all(&snapshots).map_err(|source| Error::Io {
            path: snapshots.clone(),
            source,
        })?;
        let destination = snapshots.join(snapshot_id);
        fs::rename(staging, &destination).map_err(|source| Error::Io {
            path: destination,
            source,
        })?;
        replace_current_symlink_atomic(&repo_dir, snapshot_id)
    }

    pub fn snapshot_id(now: OffsetDateTime, index_version: u64, index_bytes: &[u8]) -> String {
        let digest = Sha256::digest(index_bytes);
        format!(
            "{}-{}-{}-{}-{}",
            now.unix_timestamp_nanos(),
            std::process::id(),
            SNAPSHOT_COUNTER.fetch_add(1, Ordering::Relaxed),
            index_version,
            hex::encode(&digest[..4])
        )
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = fs::read(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_slice(&bytes).map_err(|source| Error::Json {
        path: path.to_path_buf(),
        source,
    })
}

fn replace_current_symlink_atomic(repo_dir: &Path, snapshot_id: &str) -> Result<()> {
    let tmp = repo_dir.join(format!(".current.tmp-{snapshot_id}"));
    match fs::remove_file(&tmp) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(Error::Io { path: tmp, source });
        }
    }
    std::os::unix::fs::symlink(Path::new("snapshots").join(snapshot_id), &tmp).map_err(
        |source| Error::Io {
            path: tmp.clone(),
            source,
        },
    )?;
    fs::rename(&tmp, repo_dir.join("current")).map_err(|source| Error::Io {
        path: repo_dir.join("current"),
        source,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use tempfile::tempdir;
    use time::macros::datetime;

    use super::*;

    fn make_staging(store: &RepoCacheStore, repo: &str, suffix: &str) -> PathBuf {
        let staging = store.staging_dir(repo, suffix);
        fs::create_dir_all(&staging).unwrap();
        staging
    }

    #[test]
    fn commit_replaces_current_symlink() {
        let temp = tempdir().unwrap();
        let store = RepoCacheStore::new(temp.path());

        let staging_a = make_staging(&store, "official", "a");
        store
            .commit_staging("official", &staging_a, "snap-a")
            .unwrap();
        assert_eq!(
            fs::read_link(store.repo_dir("official").join("current")).unwrap(),
            PathBuf::from("snapshots/snap-a")
        );

        let staging_b = make_staging(&store, "official", "b");
        store
            .commit_staging("official", &staging_b, "snap-b")
            .unwrap();
        assert_eq!(
            fs::read_link(store.repo_dir("official").join("current")).unwrap(),
            PathBuf::from("snapshots/snap-b")
        );
    }

    #[test]
    fn repair_state_clears_stale_etags_on_snapshot_mismatch() {
        let temp = tempdir().unwrap();
        let store = RepoCacheStore::new(temp.path());
        let staging = make_staging(&store, "official", "new");
        store.commit_staging("official", &staging, "new").unwrap();
        store
            .write_state(
                "official",
                &RepoState {
                    schema: 1,
                    repo_id: "official".to_string(),
                    current_snapshot: "old".to_string(),
                    index_etag: Some("old-index".to_string()),
                    index_bundle_etag: Some("old-bundle".to_string()),
                    last_fetched: datetime!(2026-05-07 12:00 UTC),
                    consecutive_fetch_failures: 3,
                },
            )
            .unwrap();

        let repaired = store
            .repair_state_if_needed("official", datetime!(2026-05-08 12:00 UTC))
            .unwrap()
            .unwrap();
        assert_eq!(repaired.current_snapshot, "new");
        assert_eq!(repaired.index_etag, None);
        assert_eq!(repaired.index_bundle_etag, None);
    }

    #[test]
    fn read_lock_and_write_lock_are_available() {
        let temp = tempdir().unwrap();
        let store = RepoCacheStore::new(temp.path());
        drop(store.lock("official", LockMode::Shared).unwrap());
        drop(store.lock("official", LockMode::Exclusive).unwrap());
    }

    #[test]
    fn exclusive_lock_waits_for_shared_lock_to_release() {
        let temp = tempdir().unwrap();
        let store = RepoCacheStore::new(temp.path());
        let shared = store.lock("official", LockMode::Shared).unwrap();
        let other = store.clone();
        let (attempted_tx, attempted_rx) = mpsc::channel();
        let (acquired_tx, acquired_rx) = mpsc::channel();

        let handle = thread::spawn(move || {
            attempted_tx.send(()).unwrap();
            let _lock = other.lock("official", LockMode::Exclusive).unwrap();
            acquired_tx.send(()).unwrap();
        });

        attempted_rx.recv().unwrap();
        assert!(acquired_rx
            .recv_timeout(Duration::from_millis(100))
            .is_err());
        drop(shared);
        acquired_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        handle.join().unwrap();
    }
}
