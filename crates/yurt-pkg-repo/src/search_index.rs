//! Repository search index.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};
use semver::Version;
use thiserror::Error;

use crate::metadata::PackageFile;

#[derive(Debug, Error)]
pub enum Error {
    #[error("sqlite error at {path}: {source}")]
    Sqlite {
        path: PathBuf,
        source: rusqlite::Error,
    },
    #[error("json error for package '{package}': {source}")]
    Json {
        package: String,
        source: serde_json::Error,
    },
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone)]
pub struct RepoSearchIndex {
    path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct SearchIndexes {
    repos: Vec<RepoSearchIndex>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRow {
    pub repo_id: String,
    pub name: String,
    pub latest_version: Option<String>,
    pub latest_build: Option<String>,
    pub latest_yanked: bool,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfoResult {
    pub repo_id: String,
    pub package: PackageFile,
}

impl RepoSearchIndex {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn rebuild(
        path: impl AsRef<Path>,
        repo_id: &str,
        packages: &[PackageFile],
    ) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let conn = Connection::open(&path).map_err(|source| Error::Sqlite {
            path: path.clone(),
            source,
        })?;
        conn.execute_batch(
            r#"
            PRAGMA user_version = 1;
            DROP TABLE IF EXISTS versions;
            DROP TABLE IF EXISTS packages;
            CREATE TABLE packages (
              repo_id TEXT NOT NULL,
              name TEXT NOT NULL,
              latest_version TEXT,
              latest_build TEXT,
              latest_yanked INTEGER NOT NULL,
              summary TEXT,
              package_json TEXT NOT NULL,
              PRIMARY KEY (repo_id, name)
            );
            CREATE TABLE versions (
              repo_id TEXT NOT NULL,
              name TEXT NOT NULL,
              version TEXT NOT NULL,
              build TEXT NOT NULL,
              yanked INTEGER NOT NULL,
              PRIMARY KEY (repo_id, name, version, build)
            );
            "#,
        )
        .map_err(|source| Error::Sqlite {
            path: path.clone(),
            source,
        })?;

        for package in packages {
            let mut indexed_package = package.clone();
            indexed_package
                .versions
                .sort_by(|a, b| compare_versions(b, a));
            let latest = latest_non_yanked(&indexed_package);
            let package_json =
                serde_json::to_string(&indexed_package).map_err(|source| Error::Json {
                    package: package.name.clone(),
                    source,
                })?;
            conn.execute(
                r#"
                INSERT INTO packages
                (repo_id, name, latest_version, latest_build, latest_yanked, summary, package_json)
                VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6)
                "#,
                params![
                    repo_id,
                    package.name,
                    latest.as_ref().map(|(version, _)| version.as_str()),
                    latest.as_ref().map(|(_, build)| build.as_str()),
                    latest.is_none() as i64,
                    package_json,
                ],
            )
            .map_err(|source| Error::Sqlite {
                path: path.clone(),
                source,
            })?;
            for version in &package.versions {
                conn.execute(
                    "INSERT INTO versions (repo_id, name, version, build, yanked) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![repo_id, package.name, version.version, version.build, version.yanked as i64],
                )
                .map_err(|source| Error::Sqlite {
                    path: path.clone(),
                    source,
                })?;
            }
        }
        Ok(Self { path })
    }

    pub fn search_local(&self, query: &str) -> Result<Vec<SearchRow>> {
        let conn = self.connection()?;
        let pattern = format!("%{query}%");
        let mut stmt = conn
            .prepare(
                r#"
                SELECT repo_id, name, latest_version, latest_build, latest_yanked, summary
                FROM packages
                WHERE name LIKE ?1
                ORDER BY name, repo_id
                "#,
            )
            .map_err(|source| Error::Sqlite {
                path: self.path.clone(),
                source,
            })?;
        let rows = stmt
            .query_map([pattern], |row| {
                Ok(SearchRow {
                    repo_id: row.get(0)?,
                    name: row.get(1)?,
                    latest_version: row.get(2)?,
                    latest_build: row.get(3)?,
                    latest_yanked: row.get::<_, i64>(4)? != 0,
                    summary: row.get(5)?,
                })
            })
            .map_err(|source| Error::Sqlite {
                path: self.path.clone(),
                source,
            })?;
        collect_rows(rows, &self.path)
    }

    pub fn info_local(&self, name: &str) -> Result<Option<InfoResult>> {
        let conn = self.connection()?;
        let mut stmt = conn
            .prepare("SELECT repo_id, package_json FROM packages WHERE name = ?1")
            .map_err(|source| Error::Sqlite {
                path: self.path.clone(),
                source,
            })?;
        let mut rows = stmt.query([name]).map_err(|source| Error::Sqlite {
            path: self.path.clone(),
            source,
        })?;
        let Some(row) = rows.next().map_err(|source| Error::Sqlite {
            path: self.path.clone(),
            source,
        })?
        else {
            return Ok(None);
        };
        let repo_id: String = row.get(0).map_err(|source| Error::Sqlite {
            path: self.path.clone(),
            source,
        })?;
        let json: String = row.get(1).map_err(|source| Error::Sqlite {
            path: self.path.clone(),
            source,
        })?;
        let package = serde_json::from_str(&json).map_err(|source| Error::Json {
            package: name.to_string(),
            source,
        })?;
        Ok(Some(InfoResult { repo_id, package }))
    }

    fn connection(&self) -> Result<Connection> {
        Connection::open(&self.path).map_err(|source| Error::Sqlite {
            path: self.path.clone(),
            source,
        })
    }
}

impl SearchIndexes {
    pub fn new(repos: Vec<RepoSearchIndex>) -> Self {
        Self { repos }
    }

    pub fn search(
        &self,
        query: &str,
        trusted_priorities: &BTreeMap<String, i64>,
    ) -> Result<Vec<SearchRow>> {
        let mut selected: HashMap<String, SearchRow> = HashMap::new();
        for repo in &self.repos {
            for row in repo.search_local(query)? {
                let replace = selected
                    .get(&row.name)
                    .is_none_or(|current| row_precedes(&row, current, trusted_priorities));
                if replace {
                    selected.insert(row.name.clone(), row);
                }
            }
        }
        let mut rows: Vec<_> = selected.into_values().collect();
        rows.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(rows)
    }

    pub fn info(
        &self,
        name: &str,
        repo_filter: Option<&str>,
        trusted_priorities: &BTreeMap<String, i64>,
    ) -> Result<Vec<InfoResult>> {
        let mut results = Vec::new();
        for repo in &self.repos {
            if let Some(result) = repo.info_local(name)? {
                if repo_filter.is_none_or(|filter| filter == result.repo_id) {
                    results.push(result);
                }
            }
        }
        results.sort_by(|a, b| {
            let a_pri = trusted_priorities.get(&a.repo_id).copied().unwrap_or(0);
            let b_pri = trusted_priorities.get(&b.repo_id).copied().unwrap_or(0);
            a_pri.cmp(&b_pri).then_with(|| a.repo_id.cmp(&b.repo_id))
        });
        Ok(results)
    }
}

fn row_precedes(
    candidate: &SearchRow,
    current: &SearchRow,
    trusted_priorities: &BTreeMap<String, i64>,
) -> bool {
    let candidate_priority = trusted_priorities
        .get(&candidate.repo_id)
        .copied()
        .unwrap_or(0);
    let current_priority = trusted_priorities
        .get(&current.repo_id)
        .copied()
        .unwrap_or(0);
    candidate_priority
        .cmp(&current_priority)
        .then_with(|| candidate.repo_id.cmp(&current.repo_id))
        .is_lt()
}

fn latest_non_yanked(package: &PackageFile) -> Option<(String, String)> {
    package
        .versions
        .iter()
        .filter(|version| !version.yanked)
        .max_by(|a, b| compare_versions(a, b))
        .map(|version| (version.version.clone(), version.build.clone()))
}

fn compare_versions(
    a: &crate::metadata::PackageVersion,
    b: &crate::metadata::PackageVersion,
) -> std::cmp::Ordering {
    let a_version = Version::parse(&a.version);
    let b_version = Version::parse(&b.version);
    match (a_version, b_version) {
        (Ok(a_version), Ok(b_version)) => a_version
            .cmp(&b_version)
            .then_with(|| a.build.cmp(&b.build)),
        _ => a
            .version
            .cmp(&b.version)
            .then_with(|| a.build.cmp(&b.build)),
    }
}

fn collect_rows<T>(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>>,
    path: &Path,
) -> Result<Vec<T>> {
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|source| Error::Sqlite {
            path: path.to_path_buf(),
            source,
        })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use tempfile::tempdir;
    use yurt_pkg_format::Depends;
    use yurt_pkg_trust::SigningIdentity;

    use super::*;
    use crate::metadata::PackageVersion;

    fn package(name: &str, versions: Vec<PackageVersion>) -> PackageFile {
        PackageFile {
            name: name.to_string(),
            versions,
        }
    }

    fn version(version: &str, build: &str, yanked: bool) -> PackageVersion {
        PackageVersion {
            name: None,
            version: version.to_string(),
            build: build.to_string(),
            url: "https://example.com/tool.yurtpkg".to_string(),
            sha256: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
            size: 1,
            signing: SigningIdentity {
                subject: "subject".to_string(),
                issuer: "issuer".to_string(),
            },
            depends: vec![Depends {
                name: "libc".to_string(),
                req: "^0.1".to_string(),
            }],
            yanked,
            yanked_reason: None,
        }
    }

    #[test]
    fn rebuild_selects_latest_non_yanked_with_build_tiebreak() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("db.sqlite");
        let index = RepoSearchIndex::rebuild(
            &path,
            "official",
            &[package(
                "tool",
                vec![
                    version("1.0.0", "yurt_0", false),
                    version("1.0.0", "yurt_1", false),
                    version("2.0.0", "yurt_0", true),
                ],
            )],
        )
        .unwrap();

        let rows = index.search_local("tool").unwrap();
        assert_eq!(rows[0].latest_version.as_deref(), Some("1.0.0"));
        assert_eq!(rows[0].latest_build.as_deref(), Some("yurt_1"));
    }

    #[test]
    fn search_groups_by_current_repo_priority() {
        let temp = tempdir().unwrap();
        let official = RepoSearchIndex::rebuild(
            temp.path().join("official.sqlite"),
            "official",
            &[package("tool", vec![version("1.0.0", "yurt_0", false)])],
        )
        .unwrap();
        let overlay = RepoSearchIndex::rebuild(
            temp.path().join("overlay.sqlite"),
            "overlay",
            &[package("tool", vec![version("1.0.0", "yurt_0", false)])],
        )
        .unwrap();
        let indexes = SearchIndexes::new(vec![official, overlay]);
        let priorities = BTreeMap::from([("official".to_string(), 10), ("overlay".to_string(), 0)]);

        let rows = indexes.search("tool", &priorities).unwrap();
        assert_eq!(rows[0].repo_id, "overlay");
    }

    #[test]
    fn info_returns_versions_newest_first() {
        let temp = tempdir().unwrap();
        let index = RepoSearchIndex::rebuild(
            temp.path().join("db.sqlite"),
            "official",
            &[package(
                "tool",
                vec![
                    version("1.0.0", "yurt_0", false),
                    version("1.1.0", "yurt_0", false),
                ],
            )],
        )
        .unwrap();

        let info = index.info_local("tool").unwrap().unwrap();
        assert_eq!(info.package.versions[0].version, "1.1.0");
        assert_eq!(info.package.versions[1].version, "1.0.0");
    }
}
