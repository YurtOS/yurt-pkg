use std::collections::BTreeMap;

use yurt_pkg_repo::cache::changed_packages;
use yurt_pkg_repo::metadata::RepoPackage;
use yurt_pkg_repo::select::{select_repo_for_package, Candidate};

fn pkg(hash: &str) -> RepoPackage {
    RepoPackage {
        sha256: hash.to_string(),
        size: 1,
        url: "packages/foo.json".to_string(),
    }
}

#[test]
fn package_diff_returns_new_changed_and_removed_names() {
    let mut old = BTreeMap::new();
    old.insert(
        "foo".to_string(),
        pkg("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
    );
    old.insert(
        "old".to_string(),
        pkg("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
    );

    let mut new = BTreeMap::new();
    new.insert(
        "foo".to_string(),
        pkg("cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"),
    );
    new.insert(
        "bar".to_string(),
        pkg("dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"),
    );

    let diff = changed_packages(&old, &new);
    assert_eq!(diff.changed, vec!["bar".to_string(), "foo".to_string()]);
    assert_eq!(diff.removed, vec!["old".to_string()]);
}

#[test]
fn selection_prefers_lowest_priority_then_repo_id() {
    let candidates = [
        Candidate {
            repo_id: "z".to_string(),
            priority: 10,
        },
        Candidate {
            repo_id: "a".to_string(),
            priority: 0,
        },
        Candidate {
            repo_id: "b".to_string(),
            priority: 0,
        },
    ];
    let selected = select_repo_for_package(candidates.iter()).unwrap();
    assert_eq!(selected.repo_id, "a");
}
