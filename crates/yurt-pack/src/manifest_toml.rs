//! TOML schema for `yurt-pack.toml`.
//!
//! This is the *input* to the build command, not to be confused with
//! the `info/index.json` and `info/yurt.json` files inside the resulting
//! archive. The build command translates this TOML into both manifests.

use std::collections::BTreeMap;

use serde::{de, Deserialize, Deserializer};

#[derive(Debug, Deserialize)]
pub struct PackToml {
    pub name: String,
    pub version: String,
    pub build: String,
    pub platform: String,
    pub summary: String,
    pub license: String,
    #[serde(default, deserialize_with = "deserialize_depends")]
    pub depends: BTreeMap<String, String>,
    /// Apply this uid to every walked entry unless the OS already
    /// reports a non-zero uid the author wants preserved. Defaults to 0.
    pub default_uid: Option<u32>,
    pub default_gid: Option<u32>,

    #[serde(default)]
    pub yurt: Option<YurtSection>,

    /// Hardlinks declared explicitly. Filesystems vary in how they
    /// surface inode-equality, so we don't auto-detect from the staged
    /// tree; authors point us at the canonical entry.
    #[serde(default, rename = "hardlinks")]
    pub hardlinks: Vec<Hardlink>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum DependsToml {
    Table(BTreeMap<String, String>),
    LegacyArray(Vec<String>),
}

fn deserialize_depends<'de, D>(deserializer: D) -> Result<BTreeMap<String, String>, D::Error>
where
    D: Deserializer<'de>,
{
    match DependsToml::deserialize(deserializer)? {
        DependsToml::Table(depends) => Ok(depends),
        DependsToml::LegacyArray(depends) => depends
            .into_iter()
            .map(|dep| parse_legacy_dependency(&dep).map_err(de::Error::custom))
            .collect(),
    }
}

fn parse_legacy_dependency(dep: &str) -> Result<(String, String), String> {
    let dep = dep.trim();
    if dep.is_empty() {
        return Err("legacy depends entries must not be empty".to_string());
    }
    let mut pieces = dep.splitn(2, char::is_whitespace);
    let name = pieces.next().unwrap_or_default().trim();
    let req = pieces.next().unwrap_or("*").trim();
    if name.is_empty() {
        return Err("legacy depends entries must start with a package name".to_string());
    }
    if req.is_empty() {
        return Err(format!(
            "legacy depends entry `{name}` is missing a version requirement"
        ));
    }
    Ok((name.to_string(), req.to_string()))
}

#[derive(Debug, Deserialize)]
pub struct YurtSection {
    pub min_yurt_version: Option<String>,
    #[serde(default)]
    pub requires: YurtRequires,
    #[serde(default)]
    pub commands: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct YurtRequires {
    #[serde(default)]
    pub network: bool,
    #[serde(default)]
    pub processes: bool,
    #[serde(default)]
    pub threads: bool,
}

#[derive(Debug, Deserialize)]
pub struct Hardlink {
    /// Where the hardlink lives in the package.
    pub path: String,
    /// Path inside the same package the link points at.
    pub target: String,
    pub mode: Option<u32>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
}
