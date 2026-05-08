use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use time::OffsetDateTime;
use yurt_pkg_repo::fetch::LocalFileFetcher;
use yurt_pkg_repo::metadata::Freshness;
use yurt_pkg_repo::search_index::{InfoResult, RepoSearchIndex, SearchIndexes};
use yurt_pkg_repo::state::TrustChange;
use yurt_pkg_repo::store::{LockMode, RepoCacheStore, RepoLock};
use yurt_pkg_repo::update::{UpdateEngine, UpdateOptions};
use yurt_pkg_trust::{TrustRoot, TrustedRepo, TrustedRepos};

#[derive(Debug, Parser)]
#[command(name = "pkg", about = "Yurt package client", version)]
struct Cli {
    #[arg(long, hide = true, default_value = "/etc")]
    etc_root: PathBuf,
    #[arg(long, hide = true, default_value = "/var/cache/yurt-pkg/repos")]
    cache_root: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Update,
    Search {
        query: String,
    },
    Info {
        name: String,
        #[arg(long)]
        repo: Option<String>,
    },
    Install {
        spec: String,
    },
    Upgrade {
        names: Vec<String>,
    },
    Remove {
        name: String,
    },
    List {
        #[arg(long)]
        yanked: bool,
    },
    AddRepo {
        url: String,
        #[arg(long)]
        signing_subject: String,
        #[arg(long)]
        signing_issuer: String,
        #[arg(long)]
        id: Option<String>,
        #[arg(long)]
        priority: Option<i64>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Update => update(&cli.etc_root, &cli.cache_root),
        Command::Search { query } => search(&cli.etc_root, &cli.cache_root, &query),
        Command::Info { name, repo } => {
            info(&cli.etc_root, &cli.cache_root, &name, repo.as_deref())
        }
        // TODO(docs/superpowers/specs/2026-05-07-yurt-pkg-resolver-installer-design.md):
        // implement dependency resolution, install planning, and atomic state updates.
        Command::Install { .. } | Command::Upgrade { .. } => {
            bail!("install and upgrade planning are deferred to the resolver/installer spec")
        }
        // TODO(docs/superpowers/specs/2026-05-07-yurt-pkg-resolver-installer-design.md):
        // implement installed-state mutation semantics.
        Command::Remove { .. } => bail!("remove is deferred to the resolver/installer spec"),
        // TODO(docs/superpowers/specs/2026-05-07-yurt-pkg-resolver-installer-design.md):
        // implement installed.sqlite reads.
        Command::List { .. } => bail!("list requires installed.sqlite implementation"),
        // TODO(docs/superpowers/specs/2026-05-07-yurt-pkg-update-flow-design.md):
        // implement trusted repo persistence and repo:write capability checks.
        Command::AddRepo { .. } => bail!("add-repo requires repo:write capability integration"),
    }
}

fn load_trusted_repos(etc_root: &Path) -> Result<TrustedRepos> {
    let path = etc_root.join("yurt-pkg/trusted-repos.toml");
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    TrustedRepos::from_toml_str(&text).context("failed to parse trusted repositories")
}

fn update(etc_root: &Path, cache_root: &Path) -> Result<()> {
    let trusted = load_trusted_repos(etc_root)?;
    let trust_root = TrustRoot::from_dir(etc_root.join("yurt-pkg/sigstore-trust-root"));
    let store = RepoCacheStore::new(cache_root);
    let now = OffsetDateTime::now_utc();
    let mut had_error = false;
    for repo in trusted.iter() {
        match update_one_repo(repo, trust_root.clone(), store.clone(), now) {
            Ok(outcome) => {
                if outcome.changed {
                    println!("updated {}", outcome.repo_id);
                } else {
                    println!("{} already current", outcome.repo_id);
                }
            }
            Err(err) => {
                had_error = true;
                let failures = store
                    .read_state(&repo.id)
                    .ok()
                    .flatten()
                    .map(|state| state.consecutive_fetch_failures)
                    .unwrap_or(0);
                eprintln!(
                    "failed to update {}: {err}; {failures} consecutive update failures",
                    repo.id
                );
            }
        }
    }
    if had_error {
        bail!("one or more repository updates failed");
    }
    Ok(())
}

#[cfg(feature = "test-fixtures")]
fn update_one_repo(
    repo: &TrustedRepo,
    trust_root: TrustRoot,
    store: RepoCacheStore,
    now: OffsetDateTime,
) -> yurt_pkg_repo::update::Result<yurt_pkg_repo::update::RepoUpdateOutcome> {
    let engine = UpdateEngine {
        fetcher: LocalFileFetcher,
        verifier: yurt_pkg_repo::verify::StaticVerifier {
            output: yurt_pkg_repo::verify::VerificationOutput {
                integrated_time: now,
                subject: repo.signing.subject.clone(),
                issuer: repo.signing.issuer.clone(),
            },
        },
        trust_root,
        cache_store: store,
    };
    engine.update_repo(
        repo,
        UpdateOptions {
            now,
            freshness: Freshness::default(),
        },
    )
}

#[cfg(not(feature = "test-fixtures"))]
fn update_one_repo(
    repo: &TrustedRepo,
    trust_root: TrustRoot,
    store: RepoCacheStore,
    now: OffsetDateTime,
) -> yurt_pkg_repo::update::Result<yurt_pkg_repo::update::RepoUpdateOutcome> {
    let engine = UpdateEngine {
        fetcher: LocalFileFetcher,
        verifier: yurt_pkg_repo::verify::NotImplementedVerifier,
        trust_root,
        cache_store: store,
    };
    engine.update_repo(
        repo,
        UpdateOptions {
            now,
            freshness: Freshness::default(),
        },
    )
}

fn search(etc_root: &Path, cache_root: &Path, query: &str) -> Result<()> {
    let trusted = load_trusted_repos(etc_root)?;
    let loaded = load_query_indexes(&trusted, cache_root)?;
    let rows = loaded
        .indexes
        .search(query, &loaded.priorities)
        .context("failed to query package cache")?;
    for row in rows {
        let version = match (row.latest_version, row.latest_build) {
            (Some(version), Some(build)) => format!("{version}-{build}"),
            _ => "unknown".to_string(),
        };
        println!("{} {} {}", row.name, version, row.repo_id);
    }
    drop(loaded.locks);
    Ok(())
}

fn info(etc_root: &Path, cache_root: &Path, name: &str, repo_filter: Option<&str>) -> Result<()> {
    let trusted = load_trusted_repos(etc_root)?;
    let loaded = load_query_indexes(&trusted, cache_root)?;
    let results = loaded
        .indexes
        .info(name, repo_filter, &loaded.priorities)
        .context("failed to query package cache")?;
    if results.is_empty() {
        bail!("package '{name}' not found in local cache; run pkg update");
    }
    for result in results {
        render_info(&result);
    }
    drop(loaded.locks);
    Ok(())
}

struct LoadedIndexes {
    indexes: SearchIndexes,
    priorities: BTreeMap<String, i64>,
    locks: Vec<RepoLock>,
}

fn load_query_indexes(trusted: &TrustedRepos, cache_root: &Path) -> Result<LoadedIndexes> {
    let store = RepoCacheStore::new(cache_root);
    let now = OffsetDateTime::now_utc();
    let freshness = Freshness::default();
    let mut locks = Vec::new();
    let mut indexes = Vec::new();
    let mut priorities = BTreeMap::new();

    for repo in trusted.iter() {
        priorities.insert(repo.id.clone(), repo.priority);
        let lock = store
            .lock(&repo.id, LockMode::Shared)
            .with_context(|| format!("failed to lock repo {}", repo.id))?;
        let Some(snapshot_id) = store.current_snapshot_id(&repo.id)? else {
            eprintln!("repo {} has no cache; run pkg update", repo.id);
            locks.push(lock);
            continue;
        };
        let Some(manifest) = store.read_current_manifest(&repo.id)? else {
            eprintln!("repo {} has no cache; run pkg update", repo.id);
            locks.push(lock);
            continue;
        };
        match manifest.trust_change(repo) {
            TrustChange::SigningIdentity => {
                eprintln!(
                    "trusted config for repo {} changed; run pkg update",
                    repo.id
                );
                locks.push(lock);
                continue;
            }
            TrustChange::UrlOnly => {
                eprintln!("repo {} URL changed; run pkg update to refresh it", repo.id);
            }
            TrustChange::Unchanged => {}
        }
        if now > manifest.expires_at + freshness.grace {
            eprintln!("repo {} cache is stale; run pkg update", repo.id);
        }
        if let Some(state) = store.read_state(&repo.id)? {
            if state.consecutive_fetch_failures > 0 {
                eprintln!(
                    "repo {} has {} consecutive update failures",
                    repo.id, state.consecutive_fetch_failures
                );
            }
        }
        indexes.push(RepoSearchIndex::new(
            store.snapshot_dir(&repo.id, &snapshot_id).join("db.sqlite"),
        ));
        locks.push(lock);
    }
    if indexes.is_empty() {
        bail!("no repository cache available; run pkg update");
    }
    Ok(LoadedIndexes {
        indexes: SearchIndexes::new(indexes),
        priorities,
        locks,
    })
}

fn render_info(result: &InfoResult) {
    println!("{}", result.package.name);
    println!("repo: {}", result.repo_id);
    for version in &result.package.versions {
        println!("version: {}-{}", version.version, version.build);
        println!("url: {}", version.url);
        println!("sha256: {}", version.sha256);
        println!(
            "signing: {} / {}",
            version.signing.subject, version.signing.issuer
        );
        if !version.depends.is_empty() {
            println!("depends:");
            for dep in &version.depends {
                println!("  {} {}", dep.name, dep.req);
            }
        }
    }
}
