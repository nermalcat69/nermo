use crate::concurrency::parallel_for_each;
use crate::registry::{Client, PackageMetadata};
use anyhow::{anyhow, Context, Result};
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub type PackageKey = (String, String);

/// One dependency edge: the name it's required/linked under in the
/// dependent's own `node_modules` (`local_name`), and the package it
/// actually resolves to (`key`). These differ only for npm dependency
/// aliases (`"local-name": "npm:real-name@range"` in package.json) — e.g.
/// posthog-js depends on `"web-vitals-soft-navs": "npm:web-vitals@6.2.1"` to
/// bundle a pinned copy of `web-vitals` under a different local name. For an
/// ordinary (non-aliased) dependency, `local_name == key.0`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyEdge {
    pub local_name: String,
    pub key: PackageKey,
}

/// Max concurrent registry round trips the resolver will have in flight at
/// once, mirrored per recursion level (see `Resolver::fan_out`) rather than
/// globally — see the `ponytail:` note below.
const CONCURRENCY: usize = 16;

/// How long a persisted registry metadata document is trusted before a fresh
/// resolve refetches it. Only matters for fresh resolution (no lockfile) —
/// this is what makes a *second* project resolving `react` for the first
/// time skip the network entirely instead of just skipping the store/download
/// step. The trade-off is the same one every package manager's metadata
/// cache makes: a version published in the last hour might not be seen by a
/// brand-new resolve during that window.
const METADATA_CACHE_TTL: Duration = Duration::from_secs(60 * 60);

#[derive(Serialize, Deserialize)]
struct DiskCacheEntry {
    fetched_at_unix: u64,
    metadata: PackageMetadata,
}

pub struct ResolvedPackage {
    pub tarball: String,
    pub integrity: Option<String>,
    pub shasum: Option<String>,
    /// Direct dependency edges, already resolved to exact versions.
    pub dependencies: Vec<DependencyEdge>,
}

/// The complete dependency graph: every resolved package keyed by
/// (name, version), plus the direct project-level dependencies that seeded
/// it. Two entries with the same name but different versions are expected
/// when transitive requirements conflict (see PRD §10.2).
pub struct Graph {
    pub packages: BTreeMap<PackageKey, ResolvedPackage>,
    pub roots: Vec<DependencyEdge>,
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
    disk_cache_dir: Option<PathBuf>,
}

enum DepEdge {
    Required(String, String),
    Optional(String, String),
}

impl<'a> Resolver<'a> {
    pub fn new(client: &'a Client) -> Self {
        Self {
            client,
            metadata_cache: Mutex::new(HashMap::new()),
            packages: Mutex::new(BTreeMap::new()),
            disk_cache_dir: None,
        }
    }

    /// Enable a persistent, TTL-based on-disk cache of registry metadata
    /// under `dir` (in practice `store/cache/registry/`). Without this, the
    /// global store already dedupes downloaded *content* perfectly across
    /// projects, but a second project resolving `react` for the first time
    /// still pays a fresh network round trip for its metadata — this closes
    /// that gap for the common "several projects share this package" case.
    pub fn with_disk_cache(mut self, dir: PathBuf) -> Self {
        self.disk_cache_dir = Some(dir);
        self
    }

    fn disk_cache_path(&self, name: &str) -> Option<PathBuf> {
        self.disk_cache_dir.as_ref().map(|dir| dir.join(format!("{}.json", name.replace('/', "+"))))
    }

    fn read_disk_cache(&self, name: &str) -> Option<PackageMetadata> {
        let path = self.disk_cache_path(name)?;
        let text = std::fs::read_to_string(&path).ok()?;
        let entry: DiskCacheEntry = serde_json::from_str(&text).ok()?;
        let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
        if now.saturating_sub(entry.fetched_at_unix) > METADATA_CACHE_TTL.as_secs() {
            return None;
        }
        Some(entry.metadata)
    }

    /// Best-effort: a failure to persist the cache should never fail (or
    /// even be noticed by) the resolve it's optimizing.
    fn write_disk_cache(&self, name: &str, metadata: &PackageMetadata) {
        let Some(path) = self.disk_cache_path(name) else { return };
        let Some(dir) = path.parent() else { return };
        if std::fs::create_dir_all(dir).is_err() {
            return;
        }
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        if let Ok(json) = serde_json::to_string(&DiskCacheEntry { fetched_at_unix: now, metadata: metadata.clone() }) {
            let _ = std::fs::write(&path, json);
        }
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

    /// npm dependency aliases look like `"local-name": "npm:real-name@range"`
    /// — the package actually fetched and resolved is `real-name@range`, but
    /// it's required under `local-name` (PRD §10.1: "dependency aliases").
    /// Everything below this point (metadata cache key, version picking,
    /// graph dedup key) must use the *real* name; only the edge's
    /// `local_name` should reflect what was declared in package.json.
    fn resolve_alias(local_name: &str, range: &str) -> (String, String) {
        match range.strip_prefix("npm:").and_then(|rest| rest.rsplit_once('@').filter(|(n, _)| !n.is_empty())) {
            Some((real_name, real_range)) => (real_name.to_string(), real_range.to_string()),
            None => (local_name.to_string(), range.to_string()),
        }
    }

    fn metadata(&self, name: &str) -> Result<Arc<PackageMetadata>> {
        if let Some(cached) = self.metadata_cache.lock().unwrap().get(name) {
            return Ok(cached.clone());
        }

        if let Some(disk_cached) = self.read_disk_cache(name) {
            let arc = Arc::new(disk_cached);
            let mut cache = self.metadata_cache.lock().unwrap();
            return Ok(cache.entry(name.to_string()).or_insert(arc).clone());
        }

        // Fetched outside the lock: a duplicate concurrent fetch for the
        // same name is possible (harmless, just wasted bandwidth) but never
        // blocks unrelated names behind one slow request.
        let fetched = self.client.package_metadata(name).with_context(|| format!("resolving {name}"))?;
        self.write_disk_cache(name, &fetched);
        let fetched = Arc::new(fetched);
        let mut cache = self.metadata_cache.lock().unwrap();
        Ok(cache.entry(name.to_string()).or_insert(fetched).clone())
    }

    /// Resolve a batch of dependency edges concurrently and return the
    /// resulting edges (order is not meaningful — nothing downstream depends
    /// on dependency-edge order).
    fn fan_out(&self, items: &[DepEdge]) -> Result<Vec<DependencyEdge>> {
        let resolved: Mutex<Vec<DependencyEdge>> = Mutex::new(Vec::with_capacity(items.len()));
        parallel_for_each(items, CONCURRENCY, |item| {
            let edge = match item {
                DepEdge::Required(local_name, range) => {
                    Some(DependencyEdge { local_name: local_name.clone(), key: self.resolve_one(local_name, range)? })
                }
                DepEdge::Optional(local_name, range) => self
                    .resolve_optional(local_name, range)
                    .map(|key| DependencyEdge { local_name: local_name.clone(), key }),
            };
            if let Some(edge) = edge {
                resolved.lock().unwrap().push(edge);
            }
            Ok(())
        })?;
        Ok(resolved.into_inner().unwrap())
    }

    fn resolve_one(&self, local_name: &str, range: &str) -> Result<PackageKey> {
        let (name, range) = Self::resolve_alias(local_name, range);
        let name = name.as_str();
        let range = range.as_str();

        // Reuse an already-resolved version of this package if one already
        // satisfies this edge's range too, instead of always picking a
        // fresh "highest satisfying" version independently. This is what
        // npm/pnpm/bun do to minimize distinct versions actually needed —
        // without it, two edges wanting compatible-but-different ranges for
        // the same package (one fine with an existing ^6 pin, another
        // independently resolving ^7 because nothing said not to) each mint
        // their own version, downloading more distinct packages than
        // necessary. Confirmed against a real project: this was the actual
        // cause of nermo transferring ~30% more bytes than bun for the same
        // dependency tree, not anything network-related.
        if let Some(reused) = self.find_reusable_version(name, range) {
            return Ok(reused);
        }

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

    /// Look for an already-resolved version of `name` whose version already
    /// satisfies `range`, mirroring `pick_version`'s own exact-vs-range
    /// logic so behavior stays consistent (an exact-pinned range only
    /// reuses an identical exact match; otherwise the highest already-
    /// resolved version satisfying the semver range wins, same tie-break as
    /// a fresh resolution would use).
    ///
    /// Best-effort under concurrency: two edges with different ranges for
    /// the same package can, in a narrow race window, both miss this check
    /// before either commits its own resolution, ending up with two
    /// versions where one would do. That's a missed dedup opportunity, not
    /// a correctness bug — the resolver already tolerates multiple versions
    /// of the same package coexisting (PRD §10.2).
    fn find_reusable_version(&self, name: &str, range: &str) -> Option<PackageKey> {
        let packages = self.packages.lock().unwrap();
        let mut candidates = packages.keys().filter(|(n, _)| n == name);

        if let Ok(exact) = Version::parse(range) {
            let exact = exact.to_string();
            return candidates.find(|(_, v)| *v == exact).cloned();
        }

        let req = VersionReq::parse(range).ok()?;
        candidates
            .filter_map(|key| Version::parse(&key.1).ok().map(|parsed| (key.clone(), parsed)))
            .filter(|(_, parsed)| req.matches(parsed))
            .max_by(|a, b| a.1.cmp(&b.1))
            .map(|(key, _)| key)
    }

    /// Resolve an optional dependency (npm's `optionalDependencies`), most
    /// commonly seen as one native-binary package per platform (esbuild,
    /// swc, sharp, rolldown, ...). Unlike a regular dependency, failure here
    /// is never fatal to the whole resolve: a registry lookup failure or a
    /// version that isn't built for this OS/CPU just means "not needed here"
    /// rather than a broken graph.
    fn resolve_optional(&self, local_name: &str, range: &str) -> Option<PackageKey> {
        let (name, real_range) = Self::resolve_alias(local_name, range);

        // If some other edge already resolved this exact optional
        // dependency (e.g. two different esbuild versions both needing
        // esbuild-darwin-arm64), reuse it without a second platform-check
        // round trip — it already passed the os/cpu check to get here.
        if let Some(existing) = self.find_reusable_version(&name, &real_range) {
            return Some(existing);
        }

        // Platform-variant optional dependencies are always pinned to an
        // exact version (npm generates them alongside their parent, one per
        // OS/CPU). For that overwhelmingly common case, check os/cpu via the
        // small single-version endpoint instead of the full multi-version
        // document — a real project can have ~20 platform siblings per
        // native package family, of which only one is ever a match, so this
        // is the difference between one ~2KB request and one ~100KB+ request
        // for every candidate that gets rejected. Falls back to the full
        // document only for the rare non-exact range.
        let applies_here = if let Ok(version) = Version::parse(&real_range) {
            let vmeta = self.client.version_metadata(&name, &version.to_string()).ok()?;
            platform_matches(&vmeta.os, current_os()) && platform_matches(&vmeta.cpu, current_cpu())
        } else {
            let meta = self.metadata(&name).ok()?;
            let version = pick_version(&name, &real_range, &meta).ok()?;
            let vmeta = &meta.versions[&version];
            platform_matches(&vmeta.os, current_os()) && platform_matches(&vmeta.cpu, current_cpu())
        };
        if !applies_here {
            return None;
        }
        self.resolve_one(local_name, range).ok()
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
/// Supports exact versions, Cargo/semver-style comparator ranges (`^`, `~`,
/// `>=`, `*`, ...), and OR ranges ("^0.28.0 || ^0.29.0", common in real
/// packages' peerDependencies) by splitting on `||` and matching any side.
/// Not supported: hyphen ranges ("1.2.3 - 2.3.4") and dist-tags like
/// "latest" — real npm ranges the MVP resolver still doesn't parse.
fn pick_version(name: &str, range: &str, meta: &PackageMetadata) -> Result<String> {
    if let Ok(exact) = Version::parse(range) {
        let exact = exact.to_string();
        if meta.versions.contains_key(&exact) {
            return Ok(exact);
        }
    }

    let reqs: Vec<VersionReq> = range
        .split("||")
        .map(|part| VersionReq::parse(part.trim()))
        .collect::<std::result::Result<_, _>>()
        .with_context(|| format!("unsupported version range {range:?} for {name}"))?;

    meta.versions
        .keys()
        .filter_map(|v| Version::parse(v).ok())
        .filter(|v| reqs.iter().any(|req| req.matches(v)))
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
    fn find_reusable_version_reuses_an_existing_compatible_version() {
        let client = Client::new().unwrap();
        let resolver = Resolver::new(&client);
        resolver.packages.lock().unwrap().insert(
            ("semver".to_string(), "6.3.1".to_string()),
            ResolvedPackage { tarball: String::new(), integrity: None, shasum: None, dependencies: Vec::new() },
        );

        // Real case from a real project: one consumer wants semver ^6, a
        // different consumer's ^6.0.0 range should reuse that same 6.3.1
        // instead of nermo independently resolving a fresh 7.x.
        let reused = resolver.find_reusable_version("semver", "^6.0.0");
        assert_eq!(reused, Some(("semver".to_string(), "6.3.1".to_string())));
    }

    #[test]
    fn find_reusable_version_ignores_incompatible_existing_versions() {
        let client = Client::new().unwrap();
        let resolver = Resolver::new(&client);
        resolver.packages.lock().unwrap().insert(
            ("semver".to_string(), "6.3.1".to_string()),
            ResolvedPackage { tarball: String::new(), integrity: None, shasum: None, dependencies: Vec::new() },
        );

        // ^7.0.0 is not satisfied by 6.3.1 -- must not force an incompatible reuse.
        assert!(resolver.find_reusable_version("semver", "^7.0.0").is_none());
    }

    #[test]
    fn find_reusable_version_respects_exact_pins() {
        let client = Client::new().unwrap();
        let resolver = Resolver::new(&client);
        resolver.packages.lock().unwrap().insert(
            ("esbuild-darwin-arm64".to_string(), "0.28.1".to_string()),
            ResolvedPackage { tarball: String::new(), integrity: None, shasum: None, dependencies: Vec::new() },
        );

        assert_eq!(
            resolver.find_reusable_version("esbuild-darwin-arm64", "0.28.1"),
            Some(("esbuild-darwin-arm64".to_string(), "0.28.1".to_string()))
        );
        assert!(resolver.find_reusable_version("esbuild-darwin-arm64", "0.28.2").is_none());
    }

    #[test]
    fn disk_cache_roundtrips_a_fresh_entry() {
        let dir = tempfile::tempdir().unwrap();
        let client = Client::new().unwrap();
        let resolver = Resolver::new(&client).with_disk_cache(dir.path().to_path_buf());

        let meta = fake_metadata(&["1.0.0", "1.2.0"]);
        resolver.write_disk_cache("demo", &meta);

        let cached = resolver.read_disk_cache("demo").expect("fresh entry should be readable");
        assert_eq!(cached.versions.keys().collect::<Vec<_>>(), meta.versions.keys().collect::<Vec<_>>());
    }

    #[test]
    fn disk_cache_ignores_stale_entries() {
        let dir = tempfile::tempdir().unwrap();
        let client = Client::new().unwrap();
        let resolver = Resolver::new(&client).with_disk_cache(dir.path().to_path_buf());

        let meta = fake_metadata(&["1.0.0"]);
        let stale_entry = DiskCacheEntry {
            fetched_at_unix: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
                - METADATA_CACHE_TTL.as_secs()
                - 1,
            metadata: meta,
        };
        let path = resolver.disk_cache_path("demo").unwrap();
        std::fs::write(&path, serde_json::to_string(&stale_entry).unwrap()).unwrap();

        assert!(resolver.read_disk_cache("demo").is_none(), "an expired entry must not be trusted");
    }

    #[test]
    fn disk_cache_path_is_scope_safe() {
        let dir = tempfile::tempdir().unwrap();
        let client = Client::new().unwrap();
        let resolver = Resolver::new(&client).with_disk_cache(dir.path().to_path_buf());
        let path = resolver.disk_cache_path("@types/node").unwrap();
        assert!(!path.to_string_lossy().contains('/') || path.starts_with(dir.path()));
        assert_eq!(path.file_name().unwrap().to_str().unwrap(), "@types+node.json");
    }

    #[test]
    fn resolve_alias_extracts_real_name_and_range() {
        // Real-world case: posthog-js depends on
        // "web-vitals-soft-navs": "npm:web-vitals@6.2.1" to bundle a pinned
        // web-vitals under a different local name.
        let (name, range) = Resolver::resolve_alias("web-vitals-soft-navs", "npm:web-vitals@6.2.1");
        assert_eq!(name, "web-vitals");
        assert_eq!(range, "6.2.1");
    }

    #[test]
    fn resolve_alias_handles_scoped_real_names() {
        let (name, range) = Resolver::resolve_alias("local-alias", "npm:@types/node@^22.0.0");
        assert_eq!(name, "@types/node");
        assert_eq!(range, "^22.0.0");
    }

    #[test]
    fn resolve_alias_is_a_no_op_for_ordinary_ranges() {
        let (name, range) = Resolver::resolve_alias("react", "^19.0.0");
        assert_eq!(name, "react");
        assert_eq!(range, "^19.0.0");
    }

    #[test]
    fn or_range_matches_either_side() {
        let meta = fake_metadata(&["0.28.17", "0.28.20", "0.29.0", "0.29.5", "0.30.0"]);
        // Real-world case: kysely's peerDependency on kysely-codegen-style
        // adapters looks like "^0.28.17 || ^0.29.0" — matches should span
        // both sides but stay within each caret range's ceiling.
        assert_eq!(pick_version("demo", "^0.28.17 || ^0.29.0", &meta).unwrap(), "0.29.5");
    }

    #[test]
    fn or_range_with_no_matching_side_is_an_error() {
        let meta = fake_metadata(&["1.0.0"]);
        assert!(pick_version("demo", "^2.0.0 || ^3.0.0", &meta).is_err());
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
