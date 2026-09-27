use crate::concurrency::parallel_for_each;
use crate::registry::{Client, PackageMetadata};
use anyhow::{anyhow, Context, Result};
use semver::{Version, VersionReq};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

pub type PackageKey = (String, String);

/// Max concurrent registry round trips the resolver will have in flight at
/// once, mirrored per recursion level (see `Resolver::fan_out`) rather than
/// globally — see the `ponytail:` note below.
const CONCURRENCY: usize = 16;

pub struct ResolvedPackage {
    pub tarball: String,
    pub integrity: Option<String>,
    pub shasum: Option<String>,
    /// Direct dependency edges, already resolved to exact versions.
    pub dependencies: Vec<PackageKey>,
}

/// The complete dependency graph: every resolved package keyed by
/// (name, version), plus the direct project-level dependencies that seeded
/// it. Two entries with the same name but different versions are expected
/// when transitive requirements conflict (see PRD §10.2).
pub struct Graph {
    pub packages: BTreeMap<PackageKey, ResolvedPackage>,
    pub roots: Vec<PackageKey>,
}

/// Resolves a dependency graph with concurrent registry metadata fetches.
///
/// Each node resolves its own dependency edges concurrently (`fan_out`),
/// rather than one registry round trip at a time depth-first. This is why
/// `metadata_cache` and `packages` use interior mutability instead of `&mut
/// self`/`&mut BTreeMap` threaded through the recursion: sibling edges now
/// run on separate threads and need to share both maps safely.
///
/// ponytail: concurrency is bounded *per node*, not globally across the
/// whole recursive fan-out, so a very wide-and-deep graph can transiently
/// spawn more threads than `CONCURRENCY` across the tree as a whole. Fine at
/// the graph sizes this has been run against (dozens of packages); would
/// need a real global work-stealing pool if that ever changes.
pub struct Resolver<'a> {
    client: &'a Client,
    metadata_cache: Mutex<HashMap<String, Arc<PackageMetadata>>>,
    packages: Mutex<BTreeMap<PackageKey, ResolvedPackage>>,
}

enum DepEdge {
    Required(String, String),
    Optional(String, String),
}

impl<'a> Resolver<'a> {
    pub fn new(client: &'a Client) -> Self {
        Self { client, metadata_cache: Mutex::new(HashMap::new()), packages: Mutex::new(BTreeMap::new()) }
    }

    /// Resolve a project's direct dependencies into a complete graph,
    /// recursing into transitive dependencies along the way. Consumes
    /// `self` so the final graph can be taken out of the internal mutexes
    /// without cloning it.
    pub fn resolve(self, direct: &BTreeMap<String, String>) -> Result<Graph> {
        let items: Vec<DepEdge> =
            direct.iter().map(|(name, range)| DepEdge::Required(name.clone(), range.clone())).collect();
        let roots = self.fan_out(&items)?;
        let packages = self.packages.into_inner().unwrap();
        Ok(Graph { packages, roots })
    }

    fn metadata(&self, name: &str) -> Result<Arc<PackageMetadata>> {
        if let Some(cached) = self.metadata_cache.lock().unwrap().get(name) {
            return Ok(cached.clone());
        }
        // Fetched outside the lock: a duplicate concurrent fetch for the
        // same name is possible (harmless, just wasted bandwidth) but never
        // blocks unrelated names behind one slow request.
        let fetched = Arc::new(self.client.package_metadata(name).with_context(|| format!("resolving {name}"))?);
        let mut cache = self.metadata_cache.lock().unwrap();
        Ok(cache.entry(name.to_string()).or_insert(fetched).clone())
    }

    /// Resolve a batch of dependency edges concurrently and return the
    /// resulting package keys (order is not meaningful — nothing downstream
    /// depends on dependency-edge order).
    fn fan_out(&self, items: &[DepEdge]) -> Result<Vec<PackageKey>> {
        let resolved: Mutex<Vec<PackageKey>> = Mutex::new(Vec::with_capacity(items.len()));
        parallel_for_each(items, CONCURRENCY, |item| {
            let key = match item {
                DepEdge::Required(name, range) => Some(self.resolve_one(name, range)?),
                DepEdge::Optional(name, range) => self.resolve_optional(name, range),
            };
            if let Some(key) = key {
                resolved.lock().unwrap().push(key);
            }
            Ok(())
        })?;
        Ok(resolved.into_inner().unwrap())
    }

    fn resolve_one(&self, name: &str, range: &str) -> Result<PackageKey> {
        let meta = self.metadata(name)?;
        let version = pick_version(name, range, &meta)?;
        let key: PackageKey = (name.to_string(), version.clone());

        // Check-and-claim happens under one lock acquisition, so concurrent
        // siblings that both want this same key can't both start recursing
        // into it — the second one just gets the key back once the first
        // has (eventually) filled it in. This is also how dependency cycles
        // terminate instead of recursing forever.
        {
            let mut packages = self.packages.lock().unwrap();
            if packages.contains_key(&key) {
                return Ok(key);
            }
            packages.insert(
                key.clone(),
                ResolvedPackage { tarball: String::new(), integrity: None, shasum: None, dependencies: Vec::new() },
            );
        }

        let vmeta = meta.versions[&version].clone();
        let mut items: Vec<DepEdge> = Vec::with_capacity(vmeta.dependencies.len() + vmeta.optional_dependencies.len());
        items.extend(vmeta.dependencies.iter().map(|(n, r)| DepEdge::Required(n.clone(), r.clone())));
        items.extend(vmeta.optional_dependencies.iter().map(|(n, r)| DepEdge::Optional(n.clone(), r.clone())));
        let edges = self.fan_out(&items)?;

        let mut packages = self.packages.lock().unwrap();
        let entry = packages.get_mut(&key).expect("just inserted");
        entry.tarball = vmeta.dist.tarball;
        entry.integrity = vmeta.dist.integrity;
        entry.shasum = vmeta.dist.shasum;
        entry.dependencies = edges;

        Ok(key)
    }

    /// Resolve an optional dependency (npm's `optionalDependencies`), most
    /// commonly seen as one native-binary package per platform (esbuild,
    /// swc, sharp, rolldown, ...). Unlike a regular dependency, failure here
    /// is never fatal to the whole resolve: a registry lookup failure or a
    /// version that isn't built for this OS/CPU just means "not needed here"
    /// rather than a broken graph.
    fn resolve_optional(&self, name: &str, range: &str) -> Option<PackageKey> {
        let applies_here = {
            let meta = self.metadata(name).ok()?;
            let version = pick_version(name, range, &meta).ok()?;
            let vmeta = &meta.versions[&version];
            platform_matches(&vmeta.os, current_os()) && platform_matches(&vmeta.cpu, current_cpu())
        };
        if !applies_here {
            return None;
        }
        self.resolve_one(name, range).ok()
    }
}

/// True if `list` (an npm `os`/`cpu` field, possibly with `!`-negated
/// entries) permits `current`. `None` means "no restriction".
fn platform_matches(list: &Option<Vec<String>>, current: &str) -> bool {
    let Some(values) = list else { return true };
    let mut has_allowlist_entry = false;
    for value in values {
        match value.strip_prefix('!') {
            Some(excluded) if excluded == current => return false,
            Some(_) => {}
            None => {
                has_allowlist_entry = true;
                if value == current {
                    return true;
                }
            }
        }
    }
    // Only negations were listed and none excluded us, or the list is
    // otherwise empty: treat as allowed.
    !has_allowlist_entry
}

fn current_os() -> &'static str {
    if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "windows") {
        "win32"
    } else {
        std::env::consts::OS
    }
}

fn current_cpu() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        "x86" => "ia32",
        other => other,
    }
}

/// Pick the highest published version satisfying `range`.
///
/// Supports exact versions and Cargo/semver-style comparator ranges (`^`,
/// `~`, `>=`, `*`, ...), which cover the common npm cases. Not supported:
/// hyphen ranges ("1.2.3 - 2.3.4"), OR ranges ("1.x || 2.x"), and dist-tags
/// like "latest" — real npm ranges the MVP resolver doesn't parse yet.
fn pick_version(name: &str, range: &str, meta: &PackageMetadata) -> Result<String> {
    if let Ok(exact) = Version::parse(range) {
        let exact = exact.to_string();
        if meta.versions.contains_key(&exact) {
            return Ok(exact);
        }
    }

    let req = VersionReq::parse(range).with_context(|| format!("unsupported version range {range:?} for {name}"))?;
    meta.versions
        .keys()
        .filter_map(|v| Version::parse(v).ok())
        .filter(|v| req.matches(v))
        .max()
        .map(|v| v.to_string())
        .ok_or_else(|| anyhow!("no published version of {name} satisfies {range}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{Dist, VersionMetadata};

    fn fake_metadata(versions: &[&str]) -> PackageMetadata {
        PackageMetadata {
            versions: versions
                .iter()
                .map(|v| {
                    (
                        v.to_string(),
                        VersionMetadata {
                            name: "demo".into(),
                            version: v.to_string(),
                            dist: Dist { tarball: format!("https://example/{v}.tgz"), integrity: None, shasum: None },
                            dependencies: BTreeMap::new(),
                            optional_dependencies: BTreeMap::new(),
                            os: None,
                            cpu: None,
                        },
                    )
                })
                .collect(),
        }
    }

    #[test]
    fn picks_highest_matching_caret_range() {
        let meta = fake_metadata(&["1.0.0", "1.2.0", "1.9.9", "2.0.0"]);
        assert_eq!(pick_version("demo", "^1.0.0", &meta).unwrap(), "1.9.9");
    }

    #[test]
    fn exact_version_is_preferred_verbatim() {
        let meta = fake_metadata(&["1.0.0", "1.2.0"]);
        assert_eq!(pick_version("demo", "1.2.0", &meta).unwrap(), "1.2.0");
    }

    #[test]
    fn no_matching_version_is_an_error() {
        let meta = fake_metadata(&["1.0.0"]);
        assert!(pick_version("demo", "^2.0.0", &meta).is_err());
    }

    #[test]
    fn platform_none_means_unrestricted() {
        assert!(platform_matches(&None, "darwin"));
    }

    #[test]
    fn platform_allowlist_excludes_other_platforms() {
        let list = Some(vec!["darwin".to_string(), "linux".to_string()]);
        assert!(platform_matches(&list, "darwin"));
        assert!(!platform_matches(&list, "win32"));
    }

    #[test]
    fn platform_negation_excludes_only_named_platform() {
        let list = Some(vec!["!win32".to_string()]);
        assert!(platform_matches(&list, "darwin"));
        assert!(!platform_matches(&list, "win32"));
    }

    #[test]
    fn current_cpu_maps_rust_arch_to_npm_convention() {
        // Rust's ARCH constants don't match npm's; this only checks the
        // mapping table doesn't panic and returns npm-shaped values.
        let cpu = current_cpu();
        assert!(!cpu.is_empty());
        assert_ne!(cpu, "x86_64", "should be mapped to npm's \"x64\"");
    }
}
