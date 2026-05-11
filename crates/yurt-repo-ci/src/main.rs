use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use time::{format_description::well_known::Rfc3339, Duration, OffsetDateTime};
use yurt_pkg_format::{validate_package_name, Depends};
use yurt_pkg_repo::metadata::{Index, PackageFile, PackageVersion, RepoPackage};
use yurt_pkg_trust::SigningIdentity;

const V1_SUBJECT: &str =
    "https://github.com/YurtOS/yurt-packages/.github/workflows/release.yml@refs/heads/main";
const V1_ISSUER: &str = "https://token.actions.githubusercontent.com";

#[derive(Debug, Parser)]
#[command(
    name = "yurt-repo-ci",
    about = "Yurt package repository CI helper",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    LintRecipe {
        recipe: PathBuf,
    },
    LintContinuity {
        #[arg(long)]
        package_file: PathBuf,
        #[arg(long)]
        recipe: PathBuf,
        #[arg(long)]
        allow_migration: bool,
    },
    PublishLocal {
        #[arg(long)]
        repo_root: PathBuf,
        #[arg(long)]
        artifact: PathBuf,
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        generated_at: Option<String>,
        #[arg(long)]
        reject_existing: bool,
    },
    AssertNotPublished {
        #[arg(long)]
        repo_root: PathBuf,
        #[arg(long)]
        manifest: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::LintRecipe { recipe } => lint_recipe(&recipe),
        Command::LintContinuity {
            package_file,
            recipe,
            allow_migration,
        } => lint_continuity(&package_file, &recipe, allow_migration),
        Command::PublishLocal {
            repo_root,
            artifact,
            manifest,
            generated_at,
            reject_existing,
        } => publish_local(
            &repo_root,
            &artifact,
            &manifest,
            generated_at.as_deref(),
            reject_existing,
        ),
        Command::AssertNotPublished {
            repo_root,
            manifest,
        } => assert_not_published(&repo_root, &manifest),
    }
}

fn lint_recipe(path: &PathBuf) -> Result<()> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let recipe: Recipe = toml::from_str(&text).context("parsing recipe TOML")?;
    validate_package_name(&recipe.package.name).map_err(|err| anyhow!(err))?;
    semver::Version::parse(&recipe.package.version)
        .with_context(|| format!("invalid package version '{}'", recipe.package.version))?;
    for (name, req) in &recipe.package.depends {
        validate_package_name(name).map_err(|err| anyhow!(err))?;
        semver::VersionReq::parse(req)
            .with_context(|| format!("invalid dependency requirement '{req}' for '{name}'"))?;
    }
    if recipe.package.signing.subject != V1_SUBJECT {
        bail!("v1 signing subject must be {V1_SUBJECT}");
    }
    if recipe.package.signing.issuer != V1_ISSUER {
        bail!("v1 signing issuer must be {V1_ISSUER}");
    }
    Ok(())
}

fn assert_not_published(repo_root: &Path, manifest_path: &Path) -> Result<()> {
    let manifest = read_pack_manifest(manifest_path)?;
    validate_package_name(&manifest.name).map_err(|err| anyhow!(err))?;
    semver::Version::parse(&manifest.version)
        .with_context(|| format!("invalid package version '{}'", manifest.version))?;
    let package_path = repo_root
        .join("packages")
        .join(format!("{}.json", manifest.name));
    if !package_path.is_file() {
        return Ok(());
    }
    let package = read_package_file(&package_path)?;
    ensure_version_absent(&package, &manifest)
}

fn publish_local(
    repo_root: &Path,
    artifact: &Path,
    manifest_path: &Path,
    generated_at: Option<&str>,
    reject_existing: bool,
) -> Result<()> {
    let manifest = read_pack_manifest(manifest_path)?;
    validate_package_name(&manifest.name).map_err(|err| anyhow!(err))?;
    semver::Version::parse(&manifest.version)
        .with_context(|| format!("invalid package version '{}'", manifest.version))?;

    let artifact_name = artifact
        .file_name()
        .and_then(|name| name.to_str())
        .context("artifact path must have a UTF-8 file name")?;
    let expected_artifact_name = format!(
        "{}-{}-{}.yurtpkg",
        manifest.name, manifest.version, manifest.build
    );
    if artifact_name != expected_artifact_name {
        bail!("artifact name must be {expected_artifact_name}, got {artifact_name}");
    }

    let generated_at = match generated_at {
        Some(value) => OffsetDateTime::parse(value, &Rfc3339)
            .with_context(|| format!("parsing --generated-at {value}"))?,
        None => OffsetDateTime::now_utc(),
    };
    let package_path = repo_root
        .join("packages")
        .join(format!("{}.json", manifest.name));
    let mut package = if package_path.is_file() {
        read_package_file(&package_path)?
    } else {
        PackageFile {
            name: manifest.name.clone(),
            versions: Vec::new(),
        }
    };
    if package.name != manifest.name {
        bail!(
            "package file {} declares name {}",
            package_path.display(),
            package.name
        );
    }
    if reject_existing {
        ensure_version_absent(&package, &manifest)?;
    }

    let artifact_bytes =
        fs::read(artifact).with_context(|| format!("reading {}", artifact.display()))?;
    let artifact_sha256 = sha256_hex(&artifact_bytes);
    let artifact_size = artifact_bytes.len() as u64;

    let artifact_rel = format!(
        "artifacts/{}/{}/{}",
        manifest.name, manifest.version, artifact_name
    );
    let artifact_dst = repo_root.join(&artifact_rel);
    if let Some(parent) = artifact_dst.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(&artifact_dst, &artifact_bytes)
        .with_context(|| format!("writing {}", artifact_dst.display()))?;
    fs::write(
        artifact_dst.with_extension("yurtpkg.bundle"),
        b"local test fixture bundle\n",
    )
    .with_context(|| {
        format!(
            "writing {}",
            artifact_dst.with_extension("yurtpkg.bundle").display()
        )
    })?;

    let version = PackageVersion {
        name: None,
        version: manifest.version.clone(),
        build: manifest.build.clone(),
        url: artifact_rel,
        sha256: artifact_sha256,
        size: artifact_size,
        signing: SigningIdentity {
            subject: V1_SUBJECT.to_string(),
            issuer: V1_ISSUER.to_string(),
        },
        depends: manifest.depends_vec(),
        yanked: false,
        yanked_reason: None,
    };
    package
        .versions
        .retain(|existing| existing.version != version.version || existing.build != version.build);
    package.versions.push(version);
    package.validate().map_err(|err| anyhow!(err))?;

    write_json(&package_path, &package)?;
    let package_bytes =
        fs::read(&package_path).with_context(|| format!("reading {}", package_path.display()))?;
    let package_entry = RepoPackage {
        sha256: sha256_hex(&package_bytes),
        size: package_bytes.len() as u64,
        url: format!("packages/{}.json", manifest.name),
    };

    let index_path = repo_root.join("index.json");
    let previous_version = if index_path.is_file() {
        let text = fs::read_to_string(&index_path)
            .with_context(|| format!("reading {}", index_path.display()))?;
        serde_json::from_str::<Index>(&text)
            .with_context(|| format!("parsing {}", index_path.display()))?
            .index_version
    } else {
        0
    };
    let mut packages = BTreeMap::new();
    for entry in fs::read_dir(repo_root.join("packages"))
        .with_context(|| format!("reading {}", repo_root.join("packages").display()))?
    {
        let path = entry?.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let bytes = fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        let package_file: PackageFile = serde_json::from_slice(&bytes)
            .with_context(|| format!("parsing {}", path.display()))?;
        package_file.validate().map_err(|err| anyhow!(err))?;
        packages.insert(
            package_file.name.clone(),
            RepoPackage {
                sha256: sha256_hex(&bytes),
                size: bytes.len() as u64,
                url: format!("packages/{}.json", package_file.name),
            },
        );
    }
    packages.insert(manifest.name.clone(), package_entry);

    let index = Index {
        schema: 1,
        index_version: previous_version + 1,
        generated_at,
        expires_at: generated_at + Duration::days(7),
        packages,
    };
    index
        .validate_against(None, generated_at, Default::default())
        .map_err(|err| anyhow!(err))?;
    write_json(&index_path, &index)?;
    fs::write(
        repo_root.join("index.json.bundle"),
        b"local test fixture bundle\n",
    )
    .with_context(|| format!("writing {}", repo_root.join("index.json.bundle").display()))?;

    println!(
        "published {} {}-{} to {}",
        manifest.name,
        manifest.version,
        manifest.build,
        repo_root.display()
    );
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn write_json<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let mut bytes = serde_json::to_vec_pretty(value).context("serializing JSON")?;
    bytes.push(b'\n');
    fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
}

fn read_pack_manifest(path: &Path) -> Result<PackManifest> {
    let manifest_text =
        fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&manifest_text).context("parsing yurt-pack TOML")
}

fn read_package_file(path: &Path) -> Result<PackageFile> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str::<PackageFile>(&text)
        .with_context(|| format!("parsing {}", path.display()))
}

fn ensure_version_absent(package: &PackageFile, manifest: &PackManifest) -> Result<()> {
    if package
        .versions
        .iter()
        .any(|existing| existing.version == manifest.version && existing.build == manifest.build)
    {
        bail!(
            "{} {}-{} is already published",
            manifest.name,
            manifest.version,
            manifest.build
        );
    }
    Ok(())
}

fn lint_continuity(
    package_file: &PathBuf,
    recipe_path: &PathBuf,
    allow_migration: bool,
) -> Result<()> {
    let package_text = fs::read_to_string(package_file)
        .with_context(|| format!("reading {}", package_file.display()))?;
    let package: PackageFile =
        serde_json::from_str(&package_text).context("parsing package JSON")?;
    package.validate().map_err(|err| anyhow!(err))?;

    let recipe_text = fs::read_to_string(recipe_path)
        .with_context(|| format!("reading {}", recipe_path.display()))?;
    let recipe: Recipe = toml::from_str(&recipe_text).context("parsing recipe TOML")?;
    let latest = package
        .versions
        .last()
        .context("package file has no versions")?;

    if latest.signing.subject != recipe.package.signing.subject
        || latest.signing.issuer != recipe.package.signing.issuer
    {
        if allow_migration {
            return Ok(());
        }
        bail!(
            "signer continuity violation: latest version uses {} / {}, recipe proposes {} / {}",
            latest.signing.subject,
            latest.signing.issuer,
            recipe.package.signing.subject,
            recipe.package.signing.issuer
        );
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct Recipe {
    package: Package,
}

#[derive(Debug, Deserialize)]
struct Package {
    name: String,
    version: String,
    #[serde(default)]
    depends: BTreeMap<String, String>,
    signing: Signing,
}

#[derive(Debug, Deserialize)]
struct Signing {
    subject: String,
    issuer: String,
}

#[derive(Debug, Deserialize)]
struct PackManifest {
    name: String,
    version: String,
    build: String,
    #[serde(default)]
    depends: BTreeMap<String, String>,
}

impl PackManifest {
    fn depends_vec(&self) -> Vec<Depends> {
        self.depends
            .iter()
            .map(|(name, req)| Depends {
                name: name.clone(),
                req: req.clone(),
            })
            .collect()
    }
}
