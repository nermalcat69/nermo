use crate::lockfile::{decode_key, encode_key};
use crate::resolver::PackageKey;
use crate::{archive, registry};
use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// Presence of this file in a package directory means extraction finished
/// and the package is safe to use. Its absence means the directory is a
/// leftover from an interrupted install and should be discarded.
pub(crate) const COMPLETION_MARKER: &str = ".nermo-complete";

pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn open() -> Result<Self> {
        let root = default_root()?;
        fs::create_dir_all(root.join("packages")).context("creating store/packages")?;
        fs::create_dir_all(root.join("temporary")).context("creating store/temporary")?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Construct a `Store` pointed at an arbitrary directory, for tests in
    /// other modules that need a `Store` without touching the real
    /// platform-default location.
    #[cfg(test)]
    pub(crate) fn for_test(root: &Path) -> Self {
        fs::create_dir_all(root.join("temporary")).unwrap();
        Store { root: root.to_path_buf() }
    }

    pub fn package_dir(&self, name: &str, version: &str) -> Result<PathBuf> {
        validate_component(name, true)?;
        validate_component(version, false)?;
        Ok(self.root.join("packages").join(name).join(version))
    }

    fn has_marker(&self, dest: &Path) -> bool {
        dest.join(COMPLETION_MARKER).is_file()
    }

    /// True if `name@version` is already stored and fully committed.
    pub fn has(&self, name: &str, version: &str) -> Result<bool> {
        Ok(self.has_marker(&self.package_dir(name, version)?))
    }

    /// Ensure `name@version` is present in the store, downloading and
    /// extracting it if necessary. Returns its path and whether it was
    /// already cached (a "reused" vs. "downloaded" package).
    ///
    /// `dist` is a closure, not an eager value, specifically so a caller
    /// that already knows the tarball URL and integrity hash (the resolver
    /// always does — it just fetched this exact package's metadata) pays
    /// zero extra network cost, while a caller that doesn't (`nermo fetch`,
    /// resolving an ad-hoc package with no graph behind it) only pays for a
    /// registry lookup when the package isn't already cached.
    pub fn ensure(
        &self,
        client: &registry::Client,
        name: &str,
        version: &str,
        dist: impl FnOnce() -> Result<registry::Dist>,
    ) -> Result<(PathBuf, bool)> {
        let dest = self.package_dir(name, version)?;
        if self.has_marker(&dest) {
            return Ok((dest, true));
        }
        // A directory without a marker is a leftover from a previous crash
        // or interrupted install; it's ours to discard.
        if dest.exists() {
            fs::remove_dir_all(&dest).with_context(|| format!("clearing stale partial install at {}", dest.display()))?;
        }

        let dist = dist()?;
        let bytes = client.download_tarball(&dist.tarball)?;
        archive::verify(&bytes, &dist)?;

        let tmp = tempfile::tempdir_in(self.root.join("temporary")).context("creating temp extraction dir")?;
        archive::extract(&bytes, tmp.path())?;
        fs::write(tmp.path().join(COMPLETION_MARKER), dist.integrity.as_deref().unwrap_or(""))
            .context("writing completion marker")?;
        lock_permissions(tmp.path()).context("locking package contents read-only")?;

        self.commit(tmp, &dest)?;
        Ok((dest, false))
    }

    /// Ensure many already-resolved packages are present, downloading
    /// missing ones concurrently. A cold install spends almost all of its
    /// time waiting on round trips to the registry, not CPU, so this is one
    /// of the biggest levers for install speed (PRD §15.1/§15.3). `ensure`'s
    /// own commit logic is already race-safe (Phase 3), so no additional
    /// locking is needed to run it from multiple threads. Each entry already
    /// carries its resolved `Dist`, so this makes zero metadata requests —
    /// only tarball downloads for whatever isn't already cached.
    pub fn ensure_all(
        &self,
        client: &registry::Client,
        packages: &[(PackageKey, registry::Dist)],
        concurrency: usize,
    ) -> Result<usize> {
        let downloaded = std::sync::atomic::AtomicUsize::new(0);
        crate::concurrency::parallel_for_each(packages, concurrency, |((name, version), dist)| {
            let (_, reused) = self.ensure(client, name, version, || Ok(dist.clone()))?;
            if !reused {
                downloaded.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            Ok(())
        })?;
        Ok(downloaded.into_inner())
    }

    /// Atomically move a completed temp extraction into its final store
    /// location. No lock is taken: if a concurrent install wins the race,
    /// the rename fails because `dest` is non-empty, and since `dest` then
    /// carries the marker, that result is treated as success.
    fn commit(&self, tmp: tempfile::TempDir, dest: &Path) -> Result<()> {
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        match fs::rename(tmp.path(), dest) {
            Ok(()) => Ok(()),
            Err(_) if self.has_marker(dest) => Ok(()),
            Err(e) => Err(e).with_context(|| format!("committing package into {}", dest.display())),
        }
    }

    /// Every fully-committed (name, version) currently in the store,
    /// discovered by walking for completion markers rather than trusting
    /// directory structure alone (a stale partial install has no marker).
    pub fn installed_packages(&self) -> Result<Vec<PackageKey>> {
        let packages_root = self.root.join("packages");
        let mut result = Vec::new();
        for entry in walkdir::WalkDir::new(&packages_root) {
            let entry = entry.context("walking store packages")?;
            if entry.file_name() != COMPLETION_MARKER {
                continue;
            }
            let version_dir = entry.path().parent().expect("marker has a parent directory");
            let mut components: Vec<String> = version_dir
                .strip_prefix(&packages_root)
                .context("computing package identity from store layout")?
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            if let Some(version) = components.pop() {
                result.push((components.join("/"), version));
            }
        }
        Ok(result)
    }

    /// Total size on disk of one stored package.
    pub fn package_disk_usage(&self, name: &str, version: &str) -> Result<u64> {
        dir_size(&self.package_dir(name, version)?)
    }

    /// Every installed package that no tracked project currently depends on
    /// — the shared computation behind `nermo store prune` and
    /// `nermo install --prune`. Conservative by construction: a package only
    /// drops out once *every* tracked project's most recent install stopped
    /// referencing it (e.g. after each of several projects sharing an old
    /// `next` version has been upgraded past it).
    pub fn unused_packages(&self) -> Result<Vec<PackageKey>> {
        let installed: std::collections::BTreeSet<PackageKey> = self.installed_packages()?.into_iter().collect();
        let referenced: std::collections::BTreeSet<PackageKey> =
            self.tracked_projects()?.into_values().flatten().collect();
        Ok(installed.difference(&referenced).cloned().collect())
    }

    /// Aggregate statistics for `nermo store`.
    pub fn stats(&self) -> Result<Stats> {
        let installed = self.installed_packages()?;
        let package_names: std::collections::BTreeSet<&str> = installed.iter().map(|(n, _)| n.as_str()).collect();
        Ok(Stats {
            location: self.root.clone(),
            package_names: package_names.len(),
            package_versions: installed.len(),
            disk_usage_bytes: dir_size(&self.root)?,
            projects_tracked: self.tracked_projects()?.len(),
        })
    }

    fn project_registry_path(&self) -> PathBuf {
        self.root.join("metadata").join("projects.json")
    }

    /// Record which resolved packages `project_root` depends on, so
    /// `store prune` knows not to remove them. Called after every
    /// successful install; overwrites that project's previous entry, since
    /// package.json changes mean old dependencies may no longer apply.
    pub fn track_project(&self, project_root: &Path, keys: impl Iterator<Item = PackageKey>) -> Result<()> {
        let path = self.project_registry_path();
        fs::create_dir_all(self.root.join("metadata")).context("creating store/metadata")?;

        let mut registry: BTreeMap<String, Vec<String>> = if path.is_file() {
            serde_json::from_str(&fs::read_to_string(&path)?).context("parsing store/metadata/projects.json")?
        } else {
            BTreeMap::new()
        };

        let mut encoded: Vec<String> = keys.map(|k| encode_key(&k)).collect();
        encoded.sort();
        registry.insert(project_root.to_string_lossy().into_owned(), encoded);

        let json = serde_json::to_string_pretty(&registry).context("serializing project registry")?;
        let tmp = self.root.join("metadata").join("projects.json.tmp");
        fs::write(&tmp, json).with_context(|| format!("writing {}", tmp.display()))?;
        fs::rename(&tmp, &path).with_context(|| format!("committing {}", path.display()))?;
        Ok(())
    }

    /// Every tracked project and the resolved packages it depends on.
    pub fn tracked_projects(&self) -> Result<BTreeMap<String, Vec<PackageKey>>> {
        let path = self.project_registry_path();
        if !path.is_file() {
            return Ok(BTreeMap::new());
        }
        let raw: BTreeMap<String, Vec<String>> =
            serde_json::from_str(&fs::read_to_string(&path)?).context("parsing store/metadata/projects.json")?;
        raw.into_iter()
            .map(|(project, keys)| Ok((project, keys.iter().map(|k| decode_key(k)).collect::<Result<Vec<_>>>()?)))
            .collect()
    }

    /// Delete one package from the store and return the bytes freed. Also
    /// removes now-empty parent directories (e.g. an emptied `@scope/`).
    pub fn remove_package(&self, name: &str, version: &str) -> Result<u64> {
        let dir = self.package_dir(name, version)?;
        let freed = dir_size(&dir).unwrap_or(0);
        fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))?;

        let packages_root = self.root.join("packages");
        let mut parent = dir.parent();
        while let Some(p) = parent.filter(|p| *p != packages_root) {
            if fs::remove_dir(p).is_err() {
                break; // not empty (still used by another version), or already gone
            }
            parent = p.parent();
        }
        Ok(freed)
    }
}

pub struct Stats {
    pub location: PathBuf,
    pub package_names: usize,
    pub package_versions: usize,
    pub disk_usage_bytes: u64,
    pub projects_tracked: usize,
}

fn dir_size(root: &Path) -> Result<u64> {
    let mut total = 0;
    for entry in walkdir::WalkDir::new(root) {
        let entry = entry.context("walking store for size")?;
        if entry.file_type().is_file() {
            total += entry.metadata().context("reading file metadata")?.len();
        }
    }
    Ok(total)
}

/// Make every file in the tree read-only so nothing (including a linker that
/// hardlinks these files elsewhere) can accidentally corrupt shared store
/// content across projects. Directories keep write access so future
/// operations under `root` (e.g. a later `store prune`) can still remove them.
#[cfg(unix)]
fn lock_permissions(root: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    for entry in walkdir::WalkDir::new(root) {
        let entry = entry.context("walking extracted package")?;
        if entry.file_type().is_file() {
            fs::set_permissions(entry.path(), fs::Permissions::from_mode(0o444))
                .with_context(|| format!("locking {}", entry.path().display()))?;
        }
    }
    Ok(())
}

// ponytail: Windows ACLs aren't set here; enforcing read-only on Windows
// needs a different API. Add when Windows support (Phase 9) is tackled.
#[cfg(not(unix))]
fn lock_permissions(_root: &Path) -> Result<()> {
    Ok(())
}

fn validate_component(s: &str, allow_slash: bool) -> Result<()> {
    if s.is_empty() {
        bail!("package identity component must not be empty");
    }
    if !allow_slash && s.contains('/') {
        bail!("invalid version {s:?}: must not contain '/'");
    }
    for part in s.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            bail!("invalid package identity component: {s:?}");
        }
    }
    Ok(())
}

fn nermo_home() -> Result<PathBuf> {
    if let Some(p) = env::var_os("NERMO_HOME") {
        return Ok(PathBuf::from(p));
    }
    if cfg!(target_os = "macos") {
        let home = env::var_os("HOME").context("HOME is not set")?;
        Ok(PathBuf::from(home).join("Library/Application Support/nermo"))
    } else if cfg!(target_os = "windows") {
        let local = env::var_os("LOCALAPPDATA").context("LOCALAPPDATA is not set")?;
        Ok(PathBuf::from(local).join("Nermo"))
    } else {
        let base = env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            .context("neither XDG_DATA_HOME nor HOME is set")?;
        Ok(base.join("nermo"))
    }
}

fn default_root() -> Result<PathBuf> {
    if let Some(p) = env::var_os("NERMO_STORE") {
        return Ok(PathBuf::from(p));
    }
    Ok(nermo_home()?.join("store"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_at(root: &Path) -> Store {
        fs::create_dir_all(root.join("temporary")).unwrap();
        Store { root: root.to_path_buf() }
    }

    #[test]
    fn for_test_matches_internal_layout() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::for_test(root.path());
        assert_eq!(store.root(), root.path());
    }

    #[test]
    fn rejects_traversal_in_identity_components() {
        let store = store_at(tempfile::tempdir().unwrap().path());
        assert!(store.package_dir("react", "../../etc").is_err());
        assert!(store.package_dir("../escape", "1.0.0").is_err());
        assert!(store.package_dir("@types/node", "22.0.0").is_ok());
    }

    #[test]
    fn commit_is_race_safe() {
        let root = tempfile::tempdir().unwrap();
        let store = store_at(root.path());
        let dest = store.root.join("packages/demo/1.0.0");

        let tmp1 = tempfile::tempdir_in(store.root.join("temporary")).unwrap();
        fs::write(tmp1.path().join(COMPLETION_MARKER), "x").unwrap();
        store.commit(tmp1, &dest).unwrap();
        assert!(store.has_marker(&dest));

        // A second extraction of the same package "loses the race": commit
        // must recognize the winner's result instead of erroring.
        let tmp2 = tempfile::tempdir_in(store.root.join("temporary")).unwrap();
        fs::write(tmp2.path().join(COMPLETION_MARKER), "x").unwrap();
        store.commit(tmp2, &dest).unwrap();
    }

    /// Plant a fully-committed fake package directly in the store layout,
    /// bypassing `ensure` (no network needed for these tests).
    fn plant_package(store: &Store, name: &str, version: &str, file_bytes: usize) {
        let dir = store.package_dir(name, version).unwrap();
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("index.js"), vec![b'x'; file_bytes]).unwrap();
        fs::write(dir.join(COMPLETION_MARKER), "").unwrap();
    }

    #[test]
    fn installed_packages_handles_scoped_and_unscoped_names() {
        let root = tempfile::tempdir().unwrap();
        let store = store_at(root.path());
        plant_package(&store, "react", "19.0.0", 10);
        plant_package(&store, "@types/node", "22.0.0", 10);

        let mut found = store.installed_packages().unwrap();
        found.sort();
        assert_eq!(
            found,
            vec![("@types/node".to_string(), "22.0.0".to_string()), ("react".to_string(), "19.0.0".to_string())]
        );
    }

    #[test]
    fn stats_reflects_installed_packages_and_tracked_projects() {
        let root = tempfile::tempdir().unwrap();
        let store = store_at(root.path());
        plant_package(&store, "react", "19.0.0", 100);
        plant_package(&store, "react", "18.3.1", 100);
        store.track_project(Path::new("/projects/demo"), vec![("react".to_string(), "19.0.0".to_string())].into_iter()).unwrap();

        let stats = store.stats().unwrap();
        assert_eq!(stats.package_names, 1); // two versions, one name
        assert_eq!(stats.package_versions, 2);
        assert_eq!(stats.projects_tracked, 1);
        assert!(stats.disk_usage_bytes >= 200);
    }

    #[test]
    fn remove_package_frees_space_and_cleans_empty_scope_dir() {
        let root = tempfile::tempdir().unwrap();
        let store = store_at(root.path());
        plant_package(&store, "@types/node", "22.0.0", 50);

        let freed = store.remove_package("@types/node", "22.0.0").unwrap();
        assert!(freed >= 50);
        assert!(!store.package_dir("@types/node", "22.0.0").unwrap().exists());
        // The now-empty "@types" scope directory should be cleaned up too.
        assert!(!store.root.join("packages/@types").exists());
    }

    #[test]
    fn tracked_projects_roundtrips_through_json() {
        let root = tempfile::tempdir().unwrap();
        let store = store_at(root.path());
        let keys = vec![("ms".to_string(), "2.1.3".to_string()), ("@types/node".to_string(), "22.0.0".to_string())];
        store.track_project(Path::new("/projects/demo"), keys.clone().into_iter()).unwrap();

        let tracked = store.tracked_projects().unwrap();
        let mut got = tracked.get("/projects/demo").unwrap().clone();
        got.sort();
        let mut expected = keys;
        expected.sort();
        assert_eq!(got, expected);
    }

    /// Three projects each pinned to a different `next` version; one by one
    /// they all upgrade to the same new version. A version should only
    /// become "unused" once the *last* project still on it has moved off —
    /// not the moment the first project upgrades.
    #[test]
    fn old_versions_become_unused_only_after_every_project_upgrades() {
        let root = tempfile::tempdir().unwrap();
        let store = store_at(root.path());
        for version in ["16.1.0", "16.2.0", "16.3.0"] {
            plant_package(&store, "next", version, 100);
        }

        let proj = |n: u8| PathBuf::from(format!("/projects/site-{n}"));
        let next = |v: &str| ("next".to_string(), v.to_string());

        store.track_project(&proj(1), std::iter::once(next("16.1.0"))).unwrap();
        store.track_project(&proj(2), std::iter::once(next("16.2.0"))).unwrap();
        store.track_project(&proj(3), std::iter::once(next("16.3.0"))).unwrap();
        assert!(store.unused_packages().unwrap().is_empty(), "all three old versions are still in use");

        // The upgrade target only needs to exist once someone actually
        // installs it — same as a real `nermo install` fetching it fresh.
        plant_package(&store, "next", "16.4.3", 100);

        // Project 1 upgrades: 16.1.0 is now unused (no one else needs it),
        // but 16.2.0 and 16.3.0 are still pinned by projects 2 and 3.
        store.track_project(&proj(1), std::iter::once(next("16.4.3"))).unwrap();
        assert_eq!(store.unused_packages().unwrap(), vec![next("16.1.0")]);

        // Project 2 upgrades too: now 16.1.0 and 16.2.0 are both unused.
        store.track_project(&proj(2), std::iter::once(next("16.4.3"))).unwrap();
        let mut unused = store.unused_packages().unwrap();
        unused.sort();
        assert_eq!(unused, vec![next("16.1.0"), next("16.2.0")]);

        // Project 3 upgrades last: every old version is now unused, and
        // 16.4.3 (used by all three) never appears.
        store.track_project(&proj(3), std::iter::once(next("16.4.3"))).unwrap();
        let mut unused = store.unused_packages().unwrap();
        unused.sort();
        assert_eq!(unused, vec![next("16.1.0"), next("16.2.0"), next("16.3.0")]);
    }
}
