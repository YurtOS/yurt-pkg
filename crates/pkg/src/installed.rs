use std::collections::BTreeMap;
use std::fs::File;
use std::path::Path;

use anyhow::{Context, Result};
use fs2::FileExt;
use rusqlite::{params, Connection, OptionalExtension};
use time::OffsetDateTime;
use yurt_pkg_format::{Depends, FileEntry, FileEntryKind, FilesManifest};

pub struct InstalledStore {
    conn: Connection,
}

pub struct InstallLock {
    file: File,
}

impl Drop for InstallLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledPackage {
    pub name: String,
    pub version: String,
    pub build: String,
    pub repo_id: String,
    pub source_url: String,
    pub sha256: String,
    pub size: u64,
    pub dependencies: Vec<Depends>,
}

pub struct InstalledPackageInput {
    pub name: String,
    pub version: String,
    pub build: String,
    pub repo_id: String,
    pub source_url: String,
    pub sha256: String,
    pub size: u64,
    pub index_json: String,
    pub files: Vec<FileEntry>,
    pub dependencies: Vec<Depends>,
    pub yurt_json: Option<String>,
}

impl InstalledStore {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&root)
            .with_context(|| format!("failed to create {}", root.display()))?;
        let db_path = root.join("installed.sqlite");
        let conn = Connection::open(&db_path)
            .with_context(|| format!("failed to open {}", db_path.display()))?;
        init_schema(&conn).context("failed to initialize installed package database")?;
        Ok(Self { conn })
    }

    pub fn lock(root: impl AsRef<Path>) -> Result<InstallLock> {
        let root = root.as_ref();
        std::fs::create_dir_all(root)
            .with_context(|| format!("failed to create {}", root.display()))?;
        let path = root.join(".lock");
        let file = File::options()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("failed to open {}", path.display()))?;
        file.lock_exclusive()
            .with_context(|| format!("failed to lock {}", path.display()))?;
        Ok(InstallLock { file })
    }

    pub fn recover_prepared_transactions(&self, _sandbox_root: &Path) -> Result<()> {
        let txids = self.prepared_transaction_ids()?;
        for txid in txids {
            self.conn
                .execute_batch("BEGIN IMMEDIATE")
                .context("failed to begin recovery transaction")?;
            let result = (|| -> Result<()> {
                self.conn
                    .execute(
                        "DELETE FROM files WHERE install_transaction_id = ?1",
                        [&txid],
                    )
                    .context("failed to delete prepared file rows")?;
                self.conn
                    .execute(
                        "DELETE FROM dependencies WHERE package_name IN (
                           SELECT name FROM packages WHERE install_transaction_id = ?1
                         )",
                        [&txid],
                    )
                    .context("failed to delete prepared dependency rows")?;
                self.conn
                    .execute(
                        "DELETE FROM packages WHERE install_transaction_id = ?1",
                        [&txid],
                    )
                    .context("failed to delete prepared package rows")?;
                self.conn
                    .execute(
                        "UPDATE transactions SET state = 'failed', error = ?2 WHERE id = ?1",
                        params![txid, "prepared transaction staging is missing or corrupt"],
                    )
                    .context("failed to mark prepared transaction failed")?;
                Ok(())
            })();
            match result {
                Ok(()) => self
                    .conn
                    .execute_batch("COMMIT")
                    .context("failed to commit recovery transaction")?,
                Err(err) => {
                    let _ = self.conn.execute_batch("ROLLBACK");
                    return Err(err);
                }
            }
        }
        Ok(())
    }

    pub fn list_installed(&self) -> Result<Vec<InstalledPackage>> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
                SELECT name, version, build, repo_id, source_url, sha256, size
                FROM packages
                WHERE install_state = 'installed'
                ORDER BY name
                "#,
            )
            .context("failed to prepare installed package list query")?;
        let rows = stmt
            .query_map([], |row| {
                Ok(InstalledPackage {
                    name: row.get(0)?,
                    version: row.get(1)?,
                    build: row.get(2)?,
                    repo_id: row.get(3)?,
                    source_url: row.get(4)?,
                    sha256: row.get(5)?,
                    size: row.get::<_, i64>(6)? as u64,
                    dependencies: Vec::new(),
                })
            })
            .context("failed to query installed packages")?;
        let mut packages = Vec::new();
        for row in rows {
            let mut package = row.context("failed to read installed package row")?;
            package.dependencies = self.dependencies_for(&package.name)?;
            packages.push(package);
        }
        Ok(packages)
    }

    pub fn installed_packages(&self) -> Result<BTreeMap<String, InstalledPackage>> {
        Ok(self
            .list_installed()?
            .into_iter()
            .map(|package| (package.name.clone(), package))
            .collect())
    }

    pub fn path_owner(&self, path: &str) -> Result<Option<String>> {
        self.conn
            .query_row(
                r#"
                SELECT files.package_name
                FROM files
                JOIN packages ON packages.name = files.package_name
                JOIN transactions ON transactions.id = files.install_transaction_id
                WHERE files.path = ?1
                  AND packages.install_state = 'installed'
                  AND transactions.state = 'committed'
                "#,
                [path],
                |row| row.get(0),
            )
            .optional()
            .context("failed to query installed path owner")
    }

    pub fn commit_installed(&self, txid: &str, packages: &[InstalledPackageInput]) -> Result<()> {
        let now = OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .context("failed to format install timestamp")?;
        self.conn
            .execute_batch("BEGIN IMMEDIATE")
            .context("failed to begin installed-state transaction")?;
        let result = (|| -> Result<()> {
            self.conn
                .execute(
                    r#"
                    INSERT OR REPLACE INTO transactions
                    (id, state, created_at, committed_at, error)
                    VALUES (?1, 'committed', ?2, ?2, NULL)
                    "#,
                    params![txid, now],
                )
                .context("failed to insert install transaction row")?;
            for package in packages {
                self.insert_package(txid, &now, package)?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => self
                .conn
                .execute_batch("COMMIT")
                .context("failed to commit installed-state transaction"),
            Err(err) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(err)
            }
        }
    }

    fn insert_package(
        &self,
        txid: &str,
        installed_at: &str,
        package: &InstalledPackageInput,
    ) -> Result<()> {
        let files_json = serde_json::to_string(&FilesManifest {
            files: package.files.clone(),
        })
        .context("failed to serialize installed files manifest")?;
        self.conn
            .execute(
                r#"
                INSERT OR REPLACE INTO packages
                (name, version, build, repo_id, source_url, sha256, size,
                 installed_at, install_transaction_id, install_state,
                 index_json, files_json, yurt_json)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'installed', ?10, ?11, ?12)
                "#,
                params![
                    package.name,
                    package.version,
                    package.build,
                    package.repo_id,
                    package.source_url,
                    package.sha256,
                    package.size as i64,
                    installed_at,
                    txid,
                    package.index_json,
                    files_json,
                    package.yurt_json,
                ],
            )
            .with_context(|| format!("failed to insert installed package {}", package.name))?;
        self.conn
            .execute(
                "DELETE FROM dependencies WHERE package_name = ?1",
                [&package.name],
            )
            .context("failed to replace installed dependency rows")?;
        for dependency in &package.dependencies {
            self.conn
                .execute(
                    r#"
                    INSERT INTO dependencies (package_name, dependency_name, requirement)
                    VALUES (?1, ?2, ?3)
                    "#,
                    params![package.name, dependency.name, dependency.req],
                )
                .context("failed to insert installed dependency row")?;
        }
        self.conn
            .execute("DELETE FROM files WHERE package_name = ?1", [&package.name])
            .context("failed to replace installed file rows")?;
        for file in package
            .files
            .iter()
            .filter(|file| file.kind != FileEntryKind::Dir)
        {
            self.conn
                .execute(
                    r#"
                    INSERT INTO files
                    (path, package_name, install_transaction_id, kind, sha256, target, mode, uid, gid)
                    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                    "#,
                    params![
                        file.path,
                        package.name,
                        txid,
                        file.kind.as_str(),
                        file.sha256,
                        file.target,
                        file.mode,
                        file.uid as i64,
                        file.gid as i64,
                    ],
                )
                .with_context(|| format!("failed to insert file owner for {}", file.path))?;
        }
        Ok(())
    }

    fn dependencies_for(&self, package_name: &str) -> Result<Vec<Depends>> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
                SELECT dependency_name, requirement
                FROM dependencies
                WHERE package_name = ?1
                ORDER BY dependency_name
                "#,
            )
            .context("failed to prepare dependency query")?;
        let rows = stmt
            .query_map([package_name], |row| {
                Ok(Depends {
                    name: row.get(0)?,
                    req: row.get(1)?,
                })
            })
            .context("failed to query dependencies")?;
        let mut dependencies = Vec::new();
        for row in rows {
            dependencies.push(row.context("failed to read dependency row")?);
        }
        Ok(dependencies)
    }

    fn prepared_transaction_ids(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM transactions WHERE state = 'prepared' ORDER BY id")
            .context("failed to prepare prepared transaction query")?;
        let rows = stmt
            .query_map([], |row| row.get(0))
            .context("failed to query prepared transactions")?;
        let mut txids = Vec::new();
        for row in rows {
            txids.push(row.context("failed to read prepared transaction row")?);
        }
        Ok(txids)
    }

    #[cfg(test)]
    fn record_prepared_for_test(&self, txid: &str, package: &str) -> Result<()> {
        self.conn.execute(
            r#"
            INSERT INTO transactions (id, state, created_at, committed_at, error)
            VALUES (?1, 'prepared', '2026-05-09T00:00:00Z', NULL, NULL)
            "#,
            [txid],
        )?;
        self.conn.execute(
            r#"
            INSERT INTO packages
            (name, version, build, repo_id, source_url, sha256, size,
             installed_at, install_transaction_id, install_state, index_json, files_json, yurt_json)
            VALUES (?1, '1.0.0', 'yurt_0', 'official', 'file:///tmp/foo.yurtpkg',
                    ?2, 1, '2026-05-09T00:00:00Z', ?3, 'prepared', '{}', '{"files":[]}', NULL)
            "#,
            params![package, "a".repeat(64), txid],
        )?;
        self.conn.execute(
            r#"
            INSERT INTO files
            (path, package_name, install_transaction_id, kind, sha256, target, mode, uid, gid)
            VALUES ('bin/foo', ?1, ?2, 'file', ?3, NULL, '0755', 0, 0)
            "#,
            params![package, txid, "a".repeat(64)],
        )?;
        Ok(())
    }
}

impl InstalledPackageInput {
    #[cfg(test)]
    fn new_for_test(
        name: &str,
        version: &str,
        build: &str,
        files: Vec<FileEntry>,
        dependencies: Vec<Depends>,
    ) -> Self {
        Self {
            name: name.to_string(),
            version: version.to_string(),
            build: build.to_string(),
            repo_id: "official".to_string(),
            source_url: "file:///tmp/foo.yurtpkg".to_string(),
            sha256: "a".repeat(64),
            size: 1,
            index_json: "{}".to_string(),
            files,
            dependencies,
            yurt_json: None,
        }
    }
}

fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        PRAGMA user_version = 1;

        CREATE TABLE IF NOT EXISTS transactions (
          id TEXT PRIMARY KEY,
          state TEXT NOT NULL CHECK (state IN ('prepared', 'committed', 'failed')),
          created_at TEXT NOT NULL,
          committed_at TEXT,
          error TEXT
        );

        CREATE TABLE IF NOT EXISTS packages (
          name TEXT PRIMARY KEY,
          version TEXT NOT NULL,
          build TEXT NOT NULL,
          repo_id TEXT NOT NULL,
          source_url TEXT NOT NULL,
          sha256 TEXT NOT NULL,
          size INTEGER NOT NULL,
          installed_at TEXT NOT NULL,
          install_transaction_id TEXT NOT NULL,
          install_state TEXT NOT NULL CHECK (install_state IN ('prepared', 'installed')),
          index_json TEXT NOT NULL,
          files_json TEXT NOT NULL,
          yurt_json TEXT,
          FOREIGN KEY (install_transaction_id) REFERENCES transactions(id)
        );

        CREATE TABLE IF NOT EXISTS dependencies (
          package_name TEXT NOT NULL,
          dependency_name TEXT NOT NULL,
          requirement TEXT NOT NULL,
          PRIMARY KEY (package_name, dependency_name)
        );

        CREATE TABLE IF NOT EXISTS files (
          path TEXT PRIMARY KEY,
          package_name TEXT NOT NULL,
          install_transaction_id TEXT NOT NULL,
          kind TEXT NOT NULL,
          sha256 TEXT,
          target TEXT,
          mode TEXT NOT NULL,
          uid INTEGER NOT NULL,
          gid INTEGER NOT NULL,
          FOREIGN KEY (package_name) REFERENCES packages(name),
          FOREIGN KEY (install_transaction_id) REFERENCES transactions(id)
        );

        CREATE INDEX IF NOT EXISTS transactions_state_idx ON transactions(state);
        CREATE INDEX IF NOT EXISTS packages_install_transaction_idx
          ON packages(install_transaction_id);
        CREATE INDEX IF NOT EXISTS files_install_transaction_idx
          ON files(install_transaction_id);
        "#,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use yurt_pkg_format::{Depends, FileEntry, FileEntryKind};

    #[test]
    fn init_creates_schema_and_list_ignores_prepared() {
        let temp = tempdir().unwrap();
        let store = InstalledStore::open(temp.path()).unwrap();
        store.record_prepared_for_test("tx1", "foo").unwrap();

        assert!(store.list_installed().unwrap().is_empty());
    }

    #[test]
    fn failed_recovery_removes_prepared_children() {
        let temp = tempdir().unwrap();
        let store = InstalledStore::open(temp.path()).unwrap();
        store.record_prepared_for_test("tx1", "foo").unwrap();

        store.recover_prepared_transactions(temp.path()).unwrap();

        assert!(store.list_installed().unwrap().is_empty());
        assert!(store.path_owner("bin/foo").unwrap().is_none());
    }

    #[test]
    fn committed_package_is_listed_and_owns_non_directory_paths() {
        let temp = tempdir().unwrap();
        let store = InstalledStore::open(temp.path()).unwrap();
        let files = vec![
            FileEntry {
                path: "usr".into(),
                kind: FileEntryKind::Dir,
                sha256: None,
                size: None,
                target: None,
                mode: "0755".into(),
                uid: 0,
                gid: 0,
            },
            FileEntry {
                path: "usr/bin/foo".into(),
                kind: FileEntryKind::File,
                sha256: Some("a".repeat(64)),
                size: Some(1),
                target: None,
                mode: "0755".into(),
                uid: 0,
                gid: 0,
            },
        ];
        let package = InstalledPackageInput::new_for_test(
            "foo",
            "1.0.0",
            "yurt_0",
            files,
            Vec::<Depends>::new(),
        );

        store.commit_installed("tx1", &[package]).unwrap();

        assert_eq!(store.list_installed().unwrap()[0].name, "foo");
        assert!(store.path_owner("usr").unwrap().is_none());
        assert_eq!(store.path_owner("usr/bin/foo").unwrap().unwrap(), "foo");
    }
}
