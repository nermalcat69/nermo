use crate::resolver::{Graph, PackageKey, ResolvedPackage};
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = ".nermo-lock";
const LOCKFILE_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
pub struct Lockfile {
    #[serde(rename = "lockfileVersion")]
    lockfile_version: u32,
    /// Project-declared direct dependencies as they were when this lockfile
    /// was written, keyed by name. Compared against the current manifest to
    /// detect drift before trusting the lockfile's resolution.
    direct: BTreeMap<String, DirectDependency>,
    /// Every resolved package, keyed by "name@version" ("@scope/name@version"
    /// for scoped packages).
    packages: BTreeMap<String, LockedPackage>,
}

#[derive(Debug, Serialize, Deserialize)]
struct DirectDependency {
    /// The range from package.json at lock time, e.g. "^19.0.0".
    range: String,
    /// The exact version that range resolved to.
    resolved: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LockedPackage {
    version: String,
    resolved: String,
    integrity: Option<String>,
    shasum: Option<String>,
    #[serde(default)]
    dependencies: BTreeMap<String, String>,
}

impl Lockfile {
    pub fn from_graph(direct_ranges: &BTreeMap<String, String>, graph: &Graph) -> Self {
        let direct = graph
            .roots
            .iter()
            .map(|(name, version)| {
                let range = direct_ranges.get(name).cloned().unwrap_or_default();
                (name.clone(), DirectDependency { range, resolved: version.clone() })
            })
            .collect();

        let packages = graph
            .packages
            .iter()
            .map(|(key, pkg)| {
                let dependencies = pkg.dependencies.iter().map(|(n, v)| (n.clone(), v.clone())).collect();
                (
                    encode_key(key),
                    LockedPackage {
                        version: key.1.clone(),
                        resolved: pkg.tarball.clone(),
                        integrity: pkg.integrity.clone(),
                        shasum: pkg.shasum.clone(),
                        dependencies,
                    },
                )
            })
            .collect();

        Self { lockfile_version: LOCKFILE_VERSION, direct, packages }
    }

    /// True if `direct_ranges` (freshly read from package.json) is exactly
    /// what this lockfile was resolved against, meaning its graph can be
    /// trusted without contacting the registry.
    pub fn matches_manifest(&self, direct_ranges: &BTreeMap<String, String>) -> bool {
        self.direct.len() == direct_ranges.len()
            && self.direct.iter().all(|(name, dep)| direct_ranges.get(name) == Some(&dep.range))
    }

    /// Rebuild the resolved dependency graph purely from lockfile contents,
    /// with no network access.
    pub fn to_graph(&self) -> Result<Graph> {
        let mut packages = BTreeMap::new();
        for (key_str, locked) in &self.packages {
            let key = decode_key(key_str)?;
            let dependencies = locked.dependencies.iter().map(|(n, v)| (n.clone(), v.clone())).collect();
            packages.insert(
                key,
                ResolvedPackage {
                    tarball: locked.resolved.clone(),
                    integrity: locked.integrity.clone(),
                    shasum: locked.shasum.clone(),
                    dependencies,
                },
            );
        }
        let roots = self.direct.iter().map(|(name, dep)| (name.clone(), dep.resolved.clone())).collect();
        Ok(Graph { packages, roots })
    }

    fn path(project_root: &Path) -> PathBuf {
        project_root.join(FILE_NAME)
    }

    pub fn load(project_root: &Path) -> Result<Option<Self>> {
        let path = Self::path(project_root);
        if !path.is_file() {
            return Ok(None);
        }
        let text = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let lock: Self = serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        if lock.lockfile_version != LOCKFILE_VERSION {
            bail!(
                "{} was written by an incompatible lockfile version ({}, expected {}); delete it and reinstall",
                path.display(),
                lock.lockfile_version,
                LOCKFILE_VERSION
            );
        }
        Ok(Some(lock))
    }

    /// Write the lockfile atomically, and skip the write entirely if the
    /// content wouldn't change (avoids an unnecessary rewrite per PRD §6.5).
    pub fn save(&self, project_root: &Path) -> Result<()> {
        let path = Self::path(project_root);
        let json = serde_json::to_string_pretty(self).context("serializing lockfile")?;

        if fs::read_to_string(&path).map(|existing| existing == json).unwrap_or(false) {
            return Ok(());
        }

        let tmp_path = project_root.join(format!("{FILE_NAME}.tmp"));
        fs::write(&tmp_path, &json).with_context(|| format!("writing {}", tmp_path.display()))?;
        fs::rename(&tmp_path, &path).with_context(|| format!("committing {}", path.display()))?;
        Ok(())
    }
}

/// "name@version" encoding for a package key, shared with `store` so both
/// the lockfile and the project-reference registry agree on one format.
pub(crate) fn encode_key((name, version): &PackageKey) -> String {
    format!("{name}@{version}")
}

pub(crate) fn decode_key(key: &str) -> Result<PackageKey> {
    let (name, version) = key
        .rsplit_once('@')
        .filter(|(n, _)| !n.is_empty())
        .ok_or_else(|| anyhow!("malformed lockfile package key: {key:?}"))?;
    Ok((name.to_string(), version.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_graph() -> (BTreeMap<String, String>, Graph) {
        let mut direct_ranges = BTreeMap::new();
        direct_ranges.insert("debug".to_string(), "^4.3.0".to_string());

        let mut packages = BTreeMap::new();
        packages.insert(
            ("debug".to_string(), "4.4.3".to_string()),
            ResolvedPackage {
                tarball: "https://registry.npmjs.org/debug/-/debug-4.4.3.tgz".into(),
                integrity: Some("sha512-abc".into()),
                shasum: None,
                dependencies: vec![("ms".to_string(), "2.1.3".to_string())],
            },
        );
        packages.insert(
            ("ms".to_string(), "2.1.3".to_string()),
            ResolvedPackage {
                tarball: "https://registry.npmjs.org/ms/-/ms-2.1.3.tgz".into(),
                integrity: Some("sha512-def".into()),
                shasum: None,
                dependencies: vec![],
            },
        );
        let graph = Graph { packages, roots: vec![("debug".to_string(), "4.4.3".to_string())] };
        (direct_ranges, graph)
    }

    #[test]
    fn roundtrips_through_json() {
        let (direct_ranges, graph) = sample_graph();
        let lock = Lockfile::from_graph(&direct_ranges, &graph);

        let json = serde_json::to_string(&lock).unwrap();
        let reloaded: Lockfile = serde_json::from_str(&json).unwrap();

        let rebuilt = reloaded.to_graph().unwrap();
        assert_eq!(rebuilt.packages.len(), 2);
        assert_eq!(rebuilt.roots, vec![("debug".to_string(), "4.4.3".to_string())]);
        assert_eq!(rebuilt.packages[&("debug".to_string(), "4.4.3".to_string())].dependencies, vec![(
            "ms".to_string(),
            "2.1.3".to_string()
        )]);
    }

    #[test]
    fn detects_manifest_drift() {
        let (direct_ranges, graph) = sample_graph();
        let lock = Lockfile::from_graph(&direct_ranges, &graph);

        assert!(lock.matches_manifest(&direct_ranges));

        let mut changed = direct_ranges.clone();
        changed.insert("debug".to_string(), "^5.0.0".to_string());
        assert!(!lock.matches_manifest(&changed));

        let mut extra = direct_ranges;
        extra.insert("zod".to_string(), "^3.0.0".to_string());
        assert!(!lock.matches_manifest(&extra));
    }

    #[test]
    fn scoped_package_keys_roundtrip() {
        assert_eq!(decode_key("@types/node@22.0.0").unwrap(), ("@types/node".to_string(), "22.0.0".to_string()));
        assert_eq!(encode_key(&("@types/node".to_string(), "22.0.0".to_string())), "@types/node@22.0.0");
    }
}
