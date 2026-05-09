use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{anyhow, bail, Context, Result};
use semver::{Version, VersionReq};
use yurt_pkg_format::Depends;
use yurt_pkg_repo::metadata::{PackageFile, PackageVersion};
use yurt_pkg_trust::SigningIdentity;

use crate::installed::InstalledPackage;
use crate::spec::{yurt_build_number, PackageSpec};

#[derive(Debug, Clone)]
pub struct PackageUniverse {
    records_by_name: BTreeMap<String, Vec<PackageRecord>>,
}

#[derive(Debug, Clone)]
pub struct PackageRecord {
    pub repo_id: String,
    pub priority: i64,
    pub name: String,
    pub version: String,
    pub build: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
    pub signing: SigningIdentity,
    pub depends: Vec<Depends>,
    pub yanked: bool,
}

#[derive(Debug, Clone)]
pub struct InstallPlan {
    pub to_install: Vec<PackageRecord>,
    pub reused: Vec<InstalledPackage>,
}

pub struct Resolver {
    universe: PackageUniverse,
    installed: BTreeMap<String, InstalledPackage>,
}

type SelectedPackages = BTreeMap<String, PackageRecord>;
type ReusedPackages = BTreeMap<String, InstalledPackage>;
type SolveOutput = Option<(SelectedPackages, ReusedPackages)>;

#[derive(Debug, Default, Clone)]
struct PackageRequirements {
    reqs: Vec<VersionReq>,
    exact_version: Option<Version>,
    exact_build: Option<String>,
}

impl PackageUniverse {
    pub fn from_repo_packages(repo_id: &str, priority: i64, packages: Vec<PackageFile>) -> Self {
        let mut records = Vec::new();
        for package in packages {
            for version in package.versions {
                records.push(record_from_version(
                    repo_id,
                    priority,
                    &package.name,
                    version,
                ));
            }
        }
        Self::from_records_inner(records)
    }

    pub fn merge(repos: Vec<Self>) -> Self {
        let mut records_by_name: BTreeMap<String, Vec<PackageRecord>> = BTreeMap::new();
        for repo in repos {
            for (name, records) in repo.records_by_name {
                records_by_name.entry(name).or_default().extend(records);
            }
        }
        for records in records_by_name.values_mut() {
            sort_candidates(records);
        }
        Self { records_by_name }
    }

    #[cfg(test)]
    fn from_records(records: Vec<PackageRecord>) -> Self {
        Self::from_records_inner(records)
    }

    fn from_records_inner(records: Vec<PackageRecord>) -> Self {
        let mut records_by_name: BTreeMap<String, Vec<PackageRecord>> = BTreeMap::new();
        for record in records {
            records_by_name
                .entry(record.name.clone())
                .or_default()
                .push(record);
        }
        for records in records_by_name.values_mut() {
            sort_candidates(records);
        }
        Self { records_by_name }
    }
}

fn record_from_version(
    repo_id: &str,
    priority: i64,
    name: &str,
    version: PackageVersion,
) -> PackageRecord {
    PackageRecord {
        repo_id: repo_id.to_string(),
        priority,
        name: name.to_string(),
        version: version.version,
        build: version.build,
        url: version.url,
        sha256: version.sha256,
        size: version.size,
        signing: version.signing,
        depends: version.depends,
        yanked: version.yanked,
    }
}

impl Resolver {
    pub fn new(universe: PackageUniverse, installed: BTreeMap<String, InstalledPackage>) -> Self {
        Self {
            universe,
            installed,
        }
    }

    pub fn resolve(&self, specs: &[PackageSpec]) -> Result<InstallPlan> {
        let mut requirements: BTreeMap<String, PackageRequirements> = BTreeMap::new();
        for spec in specs {
            let requirement = requirements.entry(spec.name.clone()).or_default();
            if let Some(version) = &spec.version {
                requirement.exact_version =
                    Some(Version::parse(version).context("invalid parsed package spec version")?);
            }
            if let Some(build) = &spec.build {
                requirement.exact_build = Some(build.clone());
            }
        }

        let (selected, reused) = self
            .solve(requirements, BTreeMap::new(), BTreeMap::new())?
            .context("no install plan satisfies package requirements")?;

        for (name, package) in &selected {
            ensure_dependencies_satisfied(name, package, &selected, &reused)?;
        }

        let order = install_order(&selected)?;
        let to_install = order
            .into_iter()
            .filter_map(|name| selected.get(&name).cloned())
            .collect();
        Ok(InstallPlan {
            to_install,
            reused: reused.into_values().collect(),
        })
    }

    fn solve(
        &self,
        requirements: BTreeMap<String, PackageRequirements>,
        selected: SelectedPackages,
        reused: ReusedPackages,
    ) -> Result<SolveOutput> {
        if selected_still_satisfies(&selected, &requirements).is_none() {
            return Ok(None);
        }
        for installed in reused.values() {
            if !installed_satisfies(installed, &requirements[&installed.name])? {
                return Ok(None);
            }
        }

        let Some(name) = next_unresolved(&requirements, &selected, &reused) else {
            return Ok(Some((selected, reused)));
        };

        if let Some(installed) = self.installed.get(&name) {
            ensure_installed_satisfies(installed, &requirements[&name])?;
            let mut requirements = requirements;
            for dep in &installed.dependencies {
                add_requirement(&mut requirements, dep)?;
            }
            let mut reused = reused;
            reused.insert(name, installed.clone());
            return self.solve(requirements, selected, reused);
        }

        let Some(candidates) = self.universe.records_by_name.get(&name) else {
            return Ok(None);
        };
        for candidate in candidates
            .iter()
            .filter(|candidate| candidate_satisfies(candidate, &requirements[&name]))
        {
            let mut next_requirements = requirements.clone();
            for dep in &candidate.depends {
                add_requirement(&mut next_requirements, dep)?;
            }
            let mut next_selected = selected.clone();
            next_selected.insert(name.clone(), candidate.clone());
            if let Some(solution) = self.solve(next_requirements, next_selected, reused.clone())? {
                return Ok(Some(solution));
            }
        }

        Ok(None)
    }
}

fn selected_still_satisfies(
    selected: &BTreeMap<String, PackageRecord>,
    requirements: &BTreeMap<String, PackageRequirements>,
) -> Option<()> {
    for (name, candidate) in selected {
        if !candidate_satisfies(candidate, &requirements[name]) {
            return None;
        }
    }
    Some(())
}

fn next_unresolved(
    requirements: &BTreeMap<String, PackageRequirements>,
    selected: &BTreeMap<String, PackageRecord>,
    reused: &BTreeMap<String, InstalledPackage>,
) -> Option<String> {
    requirements
        .keys()
        .find(|name| !selected.contains_key(*name) && !reused.contains_key(*name))
        .cloned()
}

fn add_requirement(
    requirements: &mut BTreeMap<String, PackageRequirements>,
    dep: &Depends,
) -> Result<()> {
    let req = VersionReq::parse(&dep.req)
        .with_context(|| format!("invalid dependency requirement {} {}", dep.name, dep.req))?;
    requirements
        .entry(dep.name.clone())
        .or_default()
        .reqs
        .push(req);
    Ok(())
}

fn ensure_installed_satisfies(
    installed: &InstalledPackage,
    requirements: &PackageRequirements,
) -> Result<()> {
    if installed_satisfies(installed, requirements)? {
        return Ok(());
    }
    bail!(
        "installed {} {}-{} conflicts with required {}",
        installed.name,
        installed.version,
        installed.build,
        describe_requirements(requirements)
    );
}

fn installed_satisfies(
    installed: &InstalledPackage,
    requirements: &PackageRequirements,
) -> Result<bool> {
    let version = Version::parse(&installed.version)
        .with_context(|| format!("installed {} has invalid version", installed.name))?;
    Ok(!(requirements
        .exact_version
        .as_ref()
        .is_some_and(|exact| exact != &version)
        || requirements
            .exact_build
            .as_ref()
            .is_some_and(|exact| exact != &installed.build)
        || !requirements.reqs.iter().all(|req| req.matches(&version))))
}

fn candidate_satisfies(candidate: &PackageRecord, requirements: &PackageRequirements) -> bool {
    if candidate.yanked {
        return false;
    }
    let Ok(version) = Version::parse(&candidate.version) else {
        return false;
    };
    if requirements
        .exact_version
        .as_ref()
        .is_some_and(|exact| exact != &version)
    {
        return false;
    }
    if requirements
        .exact_build
        .as_ref()
        .is_some_and(|exact| exact != &candidate.build)
    {
        return false;
    }
    requirements.reqs.iter().all(|req| req.matches(&version))
}

fn ensure_dependencies_satisfied(
    name: &str,
    package: &PackageRecord,
    selected: &BTreeMap<String, PackageRecord>,
    reused: &BTreeMap<String, InstalledPackage>,
) -> Result<()> {
    for dep in &package.depends {
        let req = VersionReq::parse(&dep.req)?;
        let version = selected
            .get(&dep.name)
            .map(|selected| selected.version.as_str())
            .or_else(|| {
                reused
                    .get(&dep.name)
                    .map(|installed| installed.version.as_str())
            })
            .ok_or_else(|| anyhow!("{name} depends on missing package {}", dep.name))?;
        let version = Version::parse(version)?;
        if !req.matches(&version) {
            bail!(
                "{name} dependency {} {} was not satisfied",
                dep.name,
                dep.req
            );
        }
    }
    Ok(())
}

fn install_order(selected: &BTreeMap<String, PackageRecord>) -> Result<Vec<String>> {
    let mut temporary = BTreeSet::new();
    let mut permanent = BTreeSet::new();
    let mut ordered = Vec::new();
    for name in selected.keys() {
        visit(name, selected, &mut temporary, &mut permanent, &mut ordered)?;
    }
    Ok(ordered)
}

fn visit(
    name: &str,
    selected: &BTreeMap<String, PackageRecord>,
    temporary: &mut BTreeSet<String>,
    permanent: &mut BTreeSet<String>,
    ordered: &mut Vec<String>,
) -> Result<()> {
    if permanent.contains(name) {
        return Ok(());
    }
    if !temporary.insert(name.to_string()) {
        // v1 permits cycles; lexical map iteration gives deterministic order.
        return Ok(());
    }
    if let Some(package) = selected.get(name) {
        let mut deps = package
            .depends
            .iter()
            .filter_map(|dep| {
                selected
                    .contains_key(&dep.name)
                    .then_some(dep.name.as_str())
            })
            .collect::<Vec<_>>();
        deps.sort_unstable();
        for dep in deps {
            visit(dep, selected, temporary, permanent, ordered)?;
        }
    }
    temporary.remove(name);
    permanent.insert(name.to_string());
    ordered.push(name.to_string());
    Ok(())
}

fn sort_candidates(records: &mut [PackageRecord]) {
    records.sort_by_key(|record| {
        (
            record.priority,
            record.repo_id.clone(),
            Reverse(Version::parse(&record.version).unwrap_or_else(|_| Version::new(0, 0, 0))),
            Reverse(yurt_build_number(&record.build).unwrap_or(0)),
        )
    });
}

fn describe_requirements(requirements: &PackageRequirements) -> String {
    if let Some(version) = &requirements.exact_version {
        if let Some(build) = &requirements.exact_build {
            return format!("{version}-{build}");
        }
        return version.to_string();
    }
    if requirements.reqs.is_empty() {
        "any version".to_string()
    } else {
        requirements
            .reqs
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(",")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::installed::InstalledPackage;
    use crate::spec::PackageSpec;
    use std::collections::BTreeMap;
    use yurt_pkg_format::Depends;
    use yurt_pkg_trust::SigningIdentity;

    #[test]
    fn chooses_highest_non_yanked_version_and_build() {
        let universe = PackageUniverse::from_records(vec![
            record("official", 0, "foo", "1.0.0", "yurt_0", false, &[]),
            record("official", 0, "foo", "1.0.0", "yurt_2", false, &[]),
            record("official", 0, "foo", "2.0.0", "yurt_0", true, &[]),
        ]);

        let plan = Resolver::new(universe, BTreeMap::new())
            .resolve(&[PackageSpec::parse("foo").unwrap()])
            .unwrap();

        assert_eq!(plan.to_install[0].name, "foo");
        assert_eq!(plan.to_install[0].version, "1.0.0");
        assert_eq!(plan.to_install[0].build, "yurt_2");
    }

    #[test]
    fn installs_dependencies_before_dependents() {
        let universe = PackageUniverse::from_records(vec![
            record(
                "official",
                0,
                "app",
                "1.0.0",
                "yurt_0",
                false,
                &[("lib", "^1")],
            ),
            record("official", 0, "lib", "1.2.0", "yurt_0", false, &[]),
        ]);

        let plan = Resolver::new(universe, BTreeMap::new())
            .resolve(&[PackageSpec::parse("app").unwrap()])
            .unwrap();

        assert_eq!(
            plan.to_install
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>(),
            vec!["lib", "app"]
        );
    }

    #[test]
    fn installed_version_conflict_fails_install() {
        let universe = PackageUniverse::from_records(vec![
            record(
                "official",
                0,
                "app",
                "1.0.0",
                "yurt_0",
                false,
                &[("lib", "^2")],
            ),
            record("official", 0, "lib", "2.0.0", "yurt_0", false, &[]),
        ]);
        let installed =
            BTreeMap::from([("lib".to_string(), installed("lib", "1.0.0", "yurt_0", &[]))]);

        let err = Resolver::new(universe, installed)
            .resolve(&[PackageSpec::parse("app").unwrap()])
            .unwrap_err()
            .to_string();

        assert!(err.contains("installed lib 1.0.0-yurt_0 conflicts"));
    }

    #[test]
    fn backtracks_when_later_dependency_invalidates_first_choice() {
        let universe = PackageUniverse::from_records(vec![
            record(
                "official",
                0,
                "app",
                "1.0.0",
                "yurt_0",
                false,
                &[("lib", "^1")],
            ),
            record(
                "official",
                0,
                "plugin",
                "1.0.0",
                "yurt_0",
                false,
                &[("lib", "<1.5")],
            ),
            record("official", 0, "lib", "1.9.0", "yurt_0", false, &[]),
            record("official", 0, "lib", "1.4.0", "yurt_0", false, &[]),
        ]);

        let plan = Resolver::new(universe, BTreeMap::new())
            .resolve(&[
                PackageSpec::parse("app").unwrap(),
                PackageSpec::parse("plugin").unwrap(),
            ])
            .unwrap();

        let lib = plan
            .to_install
            .iter()
            .find(|package| package.name == "lib")
            .unwrap();
        assert_eq!(lib.version, "1.4.0");
    }

    fn record(
        repo_id: &str,
        priority: i64,
        name: &str,
        version: &str,
        build: &str,
        yanked: bool,
        depends: &[(&str, &str)],
    ) -> PackageRecord {
        let depends = depends
            .iter()
            .map(|(name, req)| Depends {
                name: (*name).to_string(),
                req: (*req).to_string(),
            })
            .collect::<Vec<_>>();
        PackageRecord {
            repo_id: repo_id.to_string(),
            priority,
            name: name.to_string(),
            version: version.to_string(),
            build: build.to_string(),
            url: format!("file:///tmp/{name}-{version}-{build}.yurtpkg"),
            sha256: "a".repeat(64),
            size: 1,
            signing: SigningIdentity {
                subject: "subject".to_string(),
                issuer: "issuer".to_string(),
            },
            depends,
            yanked,
        }
    }

    fn installed(
        name: &str,
        version: &str,
        build: &str,
        depends: &[(&str, &str)],
    ) -> InstalledPackage {
        InstalledPackage {
            name: name.to_string(),
            version: version.to_string(),
            build: build.to_string(),
            repo_id: "official".to_string(),
            source_url: format!("file:///tmp/{name}-{version}-{build}.yurtpkg"),
            sha256: "a".repeat(64),
            size: 1,
            dependencies: depends
                .iter()
                .map(|(name, req)| Depends {
                    name: (*name).to_string(),
                    req: (*req).to_string(),
                })
                .collect(),
        }
    }
}
