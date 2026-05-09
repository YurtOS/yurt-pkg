use anyhow::{anyhow, Result};
use yurt_pkg_format::validate_package_name;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageSpec {
    pub name: String,
    pub version: Option<String>,
    pub build: Option<String>,
}

impl PackageSpec {
    pub fn parse(input: &str) -> Result<Self> {
        let (name, rest) = input
            .split_once('@')
            .map_or((input, None), |(name, rest)| (name, Some(rest)));
        validate_package_name(name).map_err(|err| anyhow!("{err}"))?;
        let Some(rest) = rest else {
            return Ok(Self {
                name: name.to_string(),
                version: None,
                build: None,
            });
        };
        if rest.is_empty() {
            return Err(anyhow!("invalid version in package spec '{input}'"));
        }
        let (version, build) = split_yurt_build(rest);
        semver::Version::parse(version).map_err(|err| {
            anyhow!("invalid version '{version}' in package spec '{input}': {err}")
        })?;
        Ok(Self {
            name: name.to_string(),
            version: Some(version.to_string()),
            build: build.map(ToOwned::to_owned),
        })
    }
}

fn split_yurt_build(value: &str) -> (&str, Option<&str>) {
    let Some((version, maybe_build)) = value.rsplit_once('-') else {
        return (value, None);
    };
    if yurt_build_number(maybe_build).is_some() {
        (version, Some(maybe_build))
    } else {
        (value, None)
    }
}

pub fn yurt_build_number(build: &str) -> Option<u64> {
    build.strip_prefix("yurt_")?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_unversioned_name() {
        let spec = PackageSpec::parse("busybox").unwrap();
        assert_eq!(spec.name, "busybox");
        assert_eq!(spec.version.as_deref(), None);
        assert_eq!(spec.build.as_deref(), None);
    }

    #[test]
    fn parses_semver_prerelease_as_version_not_build() {
        let spec = PackageSpec::parse("foo@1.0.0-rc.1").unwrap();
        assert_eq!(spec.name, "foo");
        assert_eq!(spec.version.as_deref(), Some("1.0.0-rc.1"));
        assert_eq!(spec.build.as_deref(), None);
    }

    #[test]
    fn parses_final_yurt_build_suffix() {
        let spec = PackageSpec::parse("foo@1.0.0-rc.1-yurt_7").unwrap();
        assert_eq!(spec.version.as_deref(), Some("1.0.0-rc.1"));
        assert_eq!(spec.build.as_deref(), Some("yurt_7"));
    }

    #[test]
    fn rejects_invalid_name() {
        let err = PackageSpec::parse("Bad@1.0.0").unwrap_err().to_string();
        assert!(err.contains("invalid package name"));
    }

    #[test]
    fn rejects_invalid_version() {
        let err = PackageSpec::parse("foo@not-semver")
            .unwrap_err()
            .to_string();
        assert!(err.contains("invalid version"));
    }
}
