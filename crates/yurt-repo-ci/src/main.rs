use std::{collections::BTreeMap, fs, path::PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand};
use serde::Deserialize;
use yurt_pkg_format::validate_package_name;
use yurt_pkg_repo::metadata::PackageFile;

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
