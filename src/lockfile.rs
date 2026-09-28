use crate::resolver::{DependencyEdge, Graph, PackageKey, ResolvedPackage};
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = ".nermo-lock";
// Bumped to 2: an existing lockfile written before the `bin` field existed
// would otherwise be silently reused as-is (`matches_manifest` only checks
// dependency ranges, not schema completeness) with every package's `bin`
// defaulting to empty — .bin shims would just never get created for a
// project that already had a lockfile, with no error or indication why.
// `load` below already hard-errors on a version mismatch rather than
// silently reinterpreting old data, so this makes that existing safety net
// actually catch this case.
const LOCKFILE_VERSION: u32 = 2;

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
    /// The resolved package's identity as "name@version". Ordinarily this
    /// starts with the same name as the map key it's stored under, but for
    /// an npm alias ("local-name": "npm:real-name@range") the two differ —
    /// storing the full identity (not just a version) is what makes that
    /// representable at all.
    resolved: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LockedPackage {
    version: String,
    resolved: String,
    integrity: Option<String>,
    shasum: Option<String>,
    /// Dependency local name -> resolved "name@version", same alias-aware
    /// shape as `DirectDependency::resolved`.
    #[serde(default)]
    dependencies: BTreeMap<String, String>,
    /// npm `bin` entries: shim name -> script path relative to the package
    /// root. Persisted so a `--frozen`/lockfile-reuse install can still set
    /// up `.bin` shims without a live registry fetch. Absent for the vast
    /// majority of packages, so skip writing an empty map to keep existing
    /// lockfiles' diffs small.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    bin: BTreeMap<String, String>,
}

impl Lockfile {
    pub fn from_graph(direct_ranges: &BTreeMap<String, String>, graph: &Graph) -> Self {
        let direct = graph
            .roots
            .iter()
            .map(|edge| {
                let range = direct_ranges.get(&edge.local_name).cloned().unwrap_or_default();
                (edge.local_name.clone(), DirectDependency { range, resolved: encode_key(&edge.key) })
            })
            .collect();

        let packages = graph
            .packages
            .iter()
            .map(|(key, pkg)| {
                let dependencies =
                    pkg.dependencies.iter().map(|edge| (edge.local_name.clone(), encode_key(&edge.key))).collect();
                (
                    encode_key(key),
                    LockedPackage {
                        version: key.1.clone(),
                        resolved: pkg.tarball.clone(),
                        integrity: pkg.integrity.clone(),
                        shasum: pkg.shasum.clone(),
                        dependencies,
                        bin: pkg.bin.clone(),
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
            let dependencies = locked
                .dependencies
                .iter()
                .map(|(local_name, resolved)| {
                    Ok(DependencyEdge { local_name: local_name.clone(), key: decode_key(resolved)? })
                })
                .collect::<Result<Vec<_>>>()?;
            packages.insert(
                key,
                ResolvedPackage {
                    tarball: locked.resolved.clone(),
                    integrity: locked.integrity.clone(),
                    shasum: locked.shasum.clone(),
                    dependencies,
                    bin: locked.bin.clone(),
                    peer_dependencies: BTreeMap::new(),
                },
            );
        }
        let roots = self
            .direct
            .iter()
            .map(|(local_name, dep)| Ok(DependencyEdge { local_name: local_name.clone(), key: decode_key(&dep.resolved)? }))
            .collect::<Result<Vec<_>>>()?;
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

    #[test]
    fn load_rejects_a_lockfile_from_an_older_schema_version() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(FILE_NAME),
            r#"{"lockfileVersion":1,"direct":{},"packages":{}}"#,
        )
        .unwrap();

        let err = Lockfile::load(dir.path()).unwrap_err();
        assert!(err.to_string().contains("delete it and reinstall"));
    }

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
                dependencies: vec![DependencyEdge {
                    local_name: "ms".to_string(),
                    key: ("ms".to_string(), "2.1.3".to_string()),
                }],
                bin: BTreeMap::new(),
                peer_dependencies: BTreeMap::new(),
            },
        );
        packages.insert(
            ("ms".to_string(), "2.1.3".to_string()),
            ResolvedPackage {
                tarball: "https://registry.npmjs.org/ms/-/ms-2.1.3.tgz".into(),
                integrity: Some("sha512-def".into()),
                shasum: None,
                dependencies: vec![],
                bin: BTreeMap::new(),
                peer_dependencies: BTreeMap::new(),
            },
        );
        let graph = Graph {
            packages,
            roots: vec![DependencyEdge {
                local_name: "debug".to_string(),
                key: ("debug".to_string(), "4.4.3".to_string()),
            }],
        };
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
        assert_eq!(
            rebuilt.roots,
            vec![DependencyEdge { local_name: "debug".to_string(), key: ("debug".to_string(), "4.4.3".to_string()) }]
        );
        assert_eq!(
            rebuilt.packages[&("debug".to_string(), "4.4.3".to_string())].dependencies,
            vec![DependencyEdge { local_name: "ms".to_string(), key: ("ms".to_string(), "2.1.3".to_string()) }]
        );
    }

    #[test]
    fn aliased_dependency_survives_the_roundtrip() {
        let mut direct_ranges = BTreeMap::new();
        direct_ranges.insert("web-vitals-soft-navs".to_string(), "npm:web-vitals@6.2.1".to_string());

        let mut packages = BTreeMap::new();
        packages.insert(
            ("web-vitals".to_string(), "6.2.1".to_string()),
            ResolvedPackage {
                tarball: "https://registry.npmjs.org/web-vitals/-/web-vitals-6.2.1.tgz".into(),
                integrity: Some("sha512-xyz".into()),
                shasum: None,
                dependencies: vec![],
                bin: BTreeMap::new(),
                peer_dependencies: BTreeMap::new(),
            },
        );
        let graph = Graph {
            packages,
            roots: vec![DependencyEdge {
                local_name: "web-vitals-soft-navs".to_string(),
                key: ("web-vitals".to_string(), "6.2.1".to_string()),
            }],
        };

        let lock = Lockfile::from_graph(&direct_ranges, &graph);
        let json = serde_json::to_string(&lock).unwrap();
        let reloaded: Lockfile = serde_json::from_str(&json).unwrap();
        let rebuilt = reloaded.to_graph().unwrap();

        assert_eq!(
            rebuilt.roots,
            vec![DependencyEdge {
                local_name: "web-vitals-soft-navs".to_string(),
                key: ("web-vitals".to_string(), "6.2.1".to_string())
            }]
        );
        assert!(rebuilt.packages.contains_key(&("web-vitals".to_string(), "6.2.1".to_string())));
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
