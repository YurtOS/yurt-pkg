use std::collections::BTreeMap;

use crate::metadata::RepoPackage;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageDiff {
    pub changed: Vec<String>,
    pub removed: Vec<String>,
}

pub fn changed_packages(
    old: &BTreeMap<String, RepoPackage>,
    new: &BTreeMap<String, RepoPackage>,
) -> PackageDiff {
    let mut changed = Vec::new();
    for (name, new_pkg) in new {
        if old.get(name).map(|old_pkg| old_pkg.sha256.as_str()) != Some(new_pkg.sha256.as_str()) {
            changed.push(name.clone());
        }
    }

    let mut removed = Vec::new();
    for name in old.keys() {
        if !new.contains_key(name) {
            removed.push(name.clone());
        }
    }

    PackageDiff { changed, removed }
}
