//! `yurt-pack` — host CLI for building `.yurtpkg` archives.
//!
//! The CLI walks a staged source tree, reads a TOML manifest describing
//! package identity and (optionally) runtime requirements, and produces
//! a single archive. The mapping is deliberately direct:
//!
//! * regular files → `EntryKind::File` with sha256 + size recorded
//! * directories  → `EntryKind::Dir`
//! * symlinks     → `EntryKind::Symlink` with the on-disk link target
//!
//! Hardlinks are not auto-detected from the source tree (filesystems on
//! the host may or may not preserve the inode-equality view). Authors
//! who want hardlinks should declare them explicitly in the manifest's
//! `[[hardlinks]]` table.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context};
use clap::{Parser, Subcommand};
use walkdir::WalkDir;

use yurt_pkg_format::{
    is_canonical_ownership, Depends, IndexManifest, RuntimeRequirements, Writer, YurtManifest,
    CANONICAL_ROOT_UID, CANONICAL_USER_UID, SCHEMA_VERSION,
};

mod manifest_toml;

#[derive(Parser)]
#[command(name = "yurt-pack", about, version)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Build a `.yurtpkg` archive from a staged source tree.
    Build {
        /// Staged source directory. Layout under here maps 1:1 to VFS paths.
        source: PathBuf,
        /// Path to the manifest TOML.
        #[arg(long, short)]
        manifest: PathBuf,
        /// Output directory. The artifact is written as
        /// `<name>-<version>-<build>.yurtpkg` inside this dir.
        #[arg(long, short)]
        out: PathBuf,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Build {
            source,
            manifest,
            out,
        } => build(&source, &manifest, &out),
    }
}

fn build(source: &Path, manifest_path: &Path, out: &Path) -> anyhow::Result<()> {
    let manifest_text = fs::read_to_string(manifest_path)
        .with_context(|| format!("reading manifest {}", manifest_path.display()))?;
    let manifest: manifest_toml::PackToml =
        toml::from_str(&manifest_text).context("parsing manifest TOML")?;

    let index = IndexManifest {
        schema_version: SCHEMA_VERSION,
        name: manifest.name.clone(),
        version: manifest.version.clone(),
        build: manifest.build.clone(),
        platform: manifest.platform.clone(),
        summary: manifest.summary.clone(),
        license: manifest.license.clone(),
        depends: manifest
            .depends
            .iter()
            .map(|(name, req)| Depends {
                name: name.clone(),
                req: req.clone(),
            })
            .collect(),
    };

    let yurt = manifest.yurt.as_ref().map(|y| YurtManifest {
        min_yurt_version: y.min_yurt_version.clone(),
        requires: RuntimeRequirements {
            network: y.requires.network,
            processes: y.requires.processes,
            threads: y.requires.threads,
        },
        commands: y.commands.clone(),
    });

    let mut writer = Writer::new(index.clone(), yurt)?;

    let canonical_source = source
        .canonicalize()
        .with_context(|| format!("canonicalizing {}", source.display()))?;

    let mut entries: Vec<(PathBuf, walkdir::DirEntry)> = Vec::new();
    for entry in WalkDir::new(&canonical_source)
        .min_depth(1)
        .sort_by_file_name()
    {
        let entry = entry.with_context(|| "walking source tree")?;
        let rel = entry
            .path()
            .strip_prefix(&canonical_source)
            .map_err(|_| anyhow!("entry escapes source root: {}", entry.path().display()))?
            .to_path_buf();
        entries.push((rel, entry));
    }

    // The package format only models two canonical users today (0:0 root,
    // 1000:1000 user). Authors must declare which one applies to the
    // staged tree rather than getting silent root-ownership by default.
    let uid = manifest.default_uid.ok_or_else(|| {
        anyhow!(
            "manifest must set default_uid (use {CANONICAL_ROOT_UID} for system tools \
             or {CANONICAL_USER_UID} for user-owned data)"
        )
    })?;
    let gid = manifest
        .default_gid
        .ok_or_else(|| anyhow!("manifest must set default_gid (typically equal to default_uid)"))?;
    if !is_canonical_ownership(uid, gid) {
        eprintln!(
            "warning: ({uid}, {gid}) is not a canonical Yurt ownership tuple. \
             The kernel only models 0:0 (root) and 1000:1000 (user) today."
        );
    }

    for (rel, entry) in &entries {
        let rel_str = rel
            .to_str()
            .ok_or_else(|| anyhow!("non-UTF-8 path: {}", rel.display()))?;
        let metadata = entry.path().symlink_metadata()?;
        let mode = metadata.permissions().mode();

        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            let target_path = fs::read_link(entry.path())
                .with_context(|| format!("reading symlink {}", entry.path().display()))?;
            // The package format is UTF-8; refuse to silently mangle a
            // non-UTF-8 symlink target with U+FFFD replacement chars.
            let target = target_path.to_str().ok_or_else(|| {
                anyhow!(
                    "symlink {} target is not valid UTF-8 (the .yurtpkg format is UTF-8 only)",
                    entry.path().display()
                )
            })?;
            writer.add_symlink(rel_str, target, mode, uid, gid)?;
        } else if file_type.is_dir() {
            writer.add_dir(rel_str, mode, uid, gid)?;
        } else if file_type.is_file() {
            let bytes = fs::read(entry.path())
                .with_context(|| format!("reading {}", entry.path().display()))?;
            writer.add_file(rel_str, bytes, mode, uid, gid)?;
        } else {
            return Err(anyhow!(
                "unsupported source entry type for {}",
                entry.path().display()
            ));
        }
    }

    for hl in &manifest.hardlinks {
        writer.add_hardlink(
            &hl.path,
            &hl.target,
            hl.mode.unwrap_or(0o755),
            hl.uid.unwrap_or(uid),
            hl.gid.unwrap_or(gid),
        )?;
    }

    fs::create_dir_all(out).with_context(|| format!("creating output dir {}", out.display()))?;
    let artifact = out.join(index.artifact_basename());
    let dst =
        fs::File::create(&artifact).with_context(|| format!("creating {}", artifact.display()))?;
    writer.finish(dst)?;

    println!("wrote {}", artifact.display());
    Ok(())
}
