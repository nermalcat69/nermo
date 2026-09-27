use crate::resolver::{Graph, PackageKey, ResolvedPackage};
use crate::store::Store;
use anyhow::{bail, Context, Result};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Marks a virtual-store entry as fully populated from the global store, so
/// repeat installs can skip re-hardlinking unchanged packages.
const ENTRY_MARKER: &str = ".nermo-linked";

/// Links a resolved dependency graph into a project's `node_modules`.
///
/// A package *with its own dependencies* is never symlinked directly from
/// the global store: Node resolves a symlinked file to its real path before
/// it looks for that module's own `node_modules`, so a plain
/// `node_modules/react -> store/...` symlink would make `react`'s internal
/// `require()`s search the immutable, dependency-free store instead of the
/// project. Such packages get a private "virtual store" directory under
/// `node_modules/.nermo/`, populated by *hardlinking* the package's files
/// (hardlinks have no separate "real" location, so this sidesteps the
/// realpath issue) plus its own `node_modules` full of directory symlinks to
/// its dependencies' virtual entries. This is the same scheme pnpm uses.
///
/// A **leaf package** (no dependencies of its own — commonly ~half a real
/// project's graph) has no internal `require()` that could hit this problem,
/// so it skips the virtual-store entry entirely and gets a direct symlink
/// straight to its store directory (`link_target`). This changes nothing
/// about disk usage (hardlinks already cost zero extra bytes either way) —
/// it just avoids walking and hardlinking every file in ~half the graph
/// during linking.
pub struct Linker<'a> {
    store: &'a Store,
    node_modules: PathBuf,
    /// When true, an unmanaged path where a link needs to go is removed
    /// instead of aborting the install. Off by default: silently deleting
    /// another tool's installed files is exactly the kind of thing PRD §6.6
    /// ("must not silently modify... or remove files") warns against, so
    /// this has to be an explicit, deliberate opt-in (`nermo install
    /// --force`), not automatic recovery.
    force: bool,
}

impl<'a> Linker<'a> {
    pub fn new(store: &'a Store, project_root: &Path, force: bool) -> Self {
        Self { store, node_modules: project_root.join("node_modules"), force }
    }

    pub fn link(&self, graph: &Graph) -> Result<()> {
        fs::create_dir_all(self.virtual_root()).context("creating node_modules/.nermo")?;

        for (key, pkg) in &graph.packages {
            if !pkg.dependencies.is_empty() {
                self.ensure_virtual_entry(key)?;
            }
        }
        for (key, pkg) in &graph.packages {
            if pkg.dependencies.is_empty() {
                continue; // a leaf has no dependencies to link into an own node_modules
            }
            let own_node_modules = self.virtual_entry_dir(key).join("node_modules");
            fs::create_dir_all(&own_node_modules)?;
            for edge in &pkg.dependencies {
                let target = self.link_target(&edge.key, &graph.packages[&edge.key])?;
                self.place_link(&own_node_modules, &edge.local_name, &target)?;
            }
        }
        let wanted: BTreeSet<&str> = graph.roots.iter().map(|edge| edge.local_name.as_str()).collect();
        for edge in &graph.roots {
            let target = self.link_target(&edge.key, &graph.packages[&edge.key])?;
            self.place_link(&self.node_modules, &edge.local_name, &target)?;
        }
        self.remove_obsolete_root_links(&wanted)?;
        Ok(())
    }

    /// Where a link to `key` should point: straight at the store for a leaf
    /// package (see the doc comment above), or at its hardlinked virtual
    /// entry otherwise.
    fn link_target(&self, key: &PackageKey, pkg: &ResolvedPackage) -> Result<PathBuf> {
        if pkg.dependencies.is_empty() {
            self.store.package_dir(&key.0, &key.1)
        } else {
            Ok(self.virtual_entry_dir(key))
        }
    }

    /// Remove top-level `node_modules` entries for packages no longer in
    /// `wanted` (e.g. after `nermo remove`, or an upgrade that drops a
    /// transitive dependency from the root set). Only ever touches entries
    /// that are themselves symlinks — the same "only what we manage" rule
    /// `place_link` already enforces — so an unmanaged file or directory a
    /// user put there is never removed.
    fn remove_obsolete_root_links(&self, wanted: &BTreeSet<&str>) -> Result<()> {
        let Ok(entries) = fs::read_dir(&self.node_modules) else { return Ok(()) };
        for entry in entries {
            let entry = entry.context("reading node_modules")?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == ".nermo" {
                continue;
            }

            if name.starts_with('@') && entry.path().is_dir() && !entry.path().is_symlink() {
                // A scope directory (e.g. "@types/"): its entries are the
                // actual managed links, named "@scope/name".
                let Ok(scoped) = fs::read_dir(entry.path()) else { continue };
                for sub in scoped {
                    let sub = sub.context("reading scoped node_modules entry")?;
                    let scoped_name = format!("{name}/{}", sub.file_name().to_string_lossy());
                    if !wanted.contains(scoped_name.as_str()) && sub.path().is_symlink() {
                        fs::remove_file(sub.path())
                            .with_context(|| format!("removing obsolete link {}", sub.path().display()))?;
                    }
                }
                // Clean up the scope directory itself once it's empty.
                if fs::read_dir(entry.path()).map(|mut d| d.next().is_none()).unwrap_or(false) {
                    let _ = fs::remove_dir(entry.path());
                }
            } else if !wanted.contains(name.as_str()) && entry.path().is_symlink() {
                fs::remove_file(entry.path())
                    .with_context(|| format!("removing obsolete link {}", entry.path().display()))?;
            }
        }
        Ok(())
    }

    fn virtual_root(&self) -> PathBuf {
        self.node_modules.join(".nermo")
    }

    fn virtual_entry_dir(&self, key: &PackageKey) -> PathBuf {
        self.virtual_root().join(encode_entry(key))
    }

    fn ensure_virtual_entry(&self, key: &PackageKey) -> Result<()> {
        let dir = self.virtual_entry_dir(key);
        if dir.join(ENTRY_MARKER).is_file() {
            return Ok(());
        }
        if dir.exists() {
            fs::remove_dir_all(&dir).with_context(|| format!("clearing stale link entry {}", dir.display()))?;
        }
        fs::create_dir_all(&dir)?;

        let store_dir = self.store.package_dir(&key.0, &key.1)?;
        hardlink_tree(&store_dir, &dir)
            .with_context(|| format!("linking {}@{} into node_modules", key.0, key.1))?;
        fs::write(dir.join(ENTRY_MARKER), "").context("writing link entry marker")?;
        Ok(())
    }

    /// Point `parent_node_modules/name` (creating scope subdirectories such
    /// as `@types/` as needed) at `target`. Refuses to touch anything that
    /// isn't already a symlink we own, so unmanaged project files are never
    /// clobbered.
    fn place_link(&self, parent_node_modules: &Path, name: &str, target: &Path) -> Result<()> {
        let link_path = parent_node_modules.join(name);
        if let Some(parent) = link_path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }

        match fs::symlink_metadata(&link_path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                fs::remove_file(&link_path).with_context(|| format!("replacing stale link {}", link_path.display()))?;
            }
            Ok(meta) if self.force => {
                if meta.is_dir() {
                    fs::remove_dir_all(&link_path)
                } else {
                    fs::remove_file(&link_path)
                }
                .with_context(|| format!("removing unmanaged path {} (--force)", link_path.display()))?;
            }
            Ok(_) => bail!(
                "refusing to overwrite unmanaged path at {}: remove it (or move it aside), or rerun with --force",
                link_path.display()
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("checking {}", link_path.display())),
        }

        symlink_dir(target, &link_path).with_context(|| format!("linking {}", link_path.display()))
    }
}

/// Encode a (name, version) pair as a single path component. Scoped names
/// contain '/', which can't appear in one component, so it's swapped for
/// '+' the same way pnpm's virtual store does ("@types+node@22.0.0").
fn encode_entry((name, version): &PackageKey) -> String {
    format!("{}@{version}", name.replace('/', "+"))
}

/// Recreate `src`'s contents under `dst` using hardlinks for regular files
/// (falling back to a copy if `src`/`dst` are on different filesystems) and
/// real symlinks for any symlinks the tarball itself contained. Skips the
/// store's internal completion marker, which is not part of the package.
fn hardlink_tree(src: &Path, dst: &Path) -> Result<()> {
    for entry in walkdir::WalkDir::new(src) {
        let entry = entry.context("walking store package")?;
        let relative = entry.path().strip_prefix(src).expect("entry is under src");
        if relative.as_os_str().is_empty() || relative == Path::new(crate::store::COMPLETION_MARKER) {
            continue;
        }
        let target = dst.join(relative);

        if entry.file_type().is_dir() {
            fs::create_dir_all(&target)?;
        } else if entry.file_type().is_symlink() {
            let link_target = fs::read_link(entry.path())?;
            symlink_dir(&link_target, &target)?;
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            if fs::hard_link(entry.path(), &target).is_err() {
                fs::copy(entry.path(), &target)
                    .with_context(|| format!("copying {}", entry.path().display()))?;
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
fn symlink_dir(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

// ponytail: plain symlink_dir needs Developer Mode or admin rights on
// Windows; a junction-based fallback (PRD §11.6) belongs here if that
// friction turns out to matter.
#[cfg(windows)]
fn symlink_dir(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_names_are_encoded_without_slashes() {
        let key = ("@types/node".to_string(), "22.0.0".to_string());
        let encoded = encode_entry(&key);
        assert_eq!(encoded, "@types+node@22.0.0");
        assert!(!encoded.contains('/'));
    }

    #[test]
    fn leaf_packages_link_straight_to_the_store() {
        let store_root = tempfile::tempdir().unwrap();
        let store = Store::for_test(store_root.path());
        let linker = Linker { store: &store, node_modules: PathBuf::from("/project/node_modules"), force: false };

        let leaf_key = ("left-pad".to_string(), "1.3.0".to_string());
        let leaf = ResolvedPackage { tarball: String::new(), integrity: None, shasum: None, dependencies: vec![] };
        let target = linker.link_target(&leaf_key, &leaf).unwrap();
        assert_eq!(target, store.package_dir("left-pad", "1.3.0").unwrap());
    }

    #[test]
    fn non_leaf_packages_link_to_the_virtual_store_entry() {
        let store_root = tempfile::tempdir().unwrap();
        let store = Store::for_test(store_root.path());
        let linker = Linker { store: &store, node_modules: PathBuf::from("/project/node_modules"), force: false };

        let key = ("debug".to_string(), "4.4.3".to_string());
        let non_leaf = ResolvedPackage {
            tarball: String::new(),
            integrity: None,
            shasum: None,
            dependencies: vec![crate::resolver::DependencyEdge {
                local_name: "ms".to_string(),
                key: ("ms".to_string(), "2.1.3".to_string()),
            }],
        };
        let target = linker.link_target(&key, &non_leaf).unwrap();
        assert_eq!(target, linker.virtual_entry_dir(&key));
        assert_ne!(target, store.package_dir("debug", "4.4.3").unwrap());
    }

    #[test]
    fn hardlink_tree_copies_files_and_skips_marker() {
        let src = tempfile::tempdir().unwrap();
        fs::create_dir_all(src.path().join("lib")).unwrap();
        fs::write(src.path().join("index.js"), "module.exports = 1;").unwrap();
        fs::write(src.path().join("lib/util.js"), "// util").unwrap();
        fs::write(src.path().join(crate::store::COMPLETION_MARKER), "sha512-x").unwrap();

        let dst = tempfile::tempdir().unwrap();
        hardlink_tree(src.path(), dst.path()).unwrap();

        assert!(dst.path().join("index.js").is_file());
        assert!(dst.path().join("lib/util.js").is_file());
        assert!(!dst.path().join(crate::store::COMPLETION_MARKER).exists());
    }

    #[test]
    fn place_link_refuses_to_clobber_unmanaged_directory() {
        let node_modules = tempfile::tempdir().unwrap();
        let unmanaged = node_modules.path().join("react");
        fs::create_dir_all(&unmanaged).unwrap();
        fs::write(unmanaged.join("index.js"), "hand-written").unwrap();

        let target = tempfile::tempdir().unwrap();
        let store_root = tempfile::tempdir().unwrap();
        let store = Store::for_test(store_root.path());
        let linker = Linker { store: &store, node_modules: node_modules.path().to_path_buf(), force: false };

        let err = linker.place_link(node_modules.path(), "react", target.path()).unwrap_err();
        assert!(err.to_string().contains("unmanaged"));
        assert!(unmanaged.join("index.js").is_file(), "unmanaged file must survive");
    }

    #[test]
    fn place_link_with_force_removes_unmanaged_directory() {
        let node_modules = tempfile::tempdir().unwrap();
        let unmanaged = node_modules.path().join("react");
        fs::create_dir_all(&unmanaged).unwrap();
        fs::write(unmanaged.join("index.js"), "bun-installed").unwrap();

        let target = tempfile::tempdir().unwrap();
        let store_root = tempfile::tempdir().unwrap();
        let store = Store::for_test(store_root.path());
        let linker = Linker { store: &store, node_modules: node_modules.path().to_path_buf(), force: true };

        linker.place_link(node_modules.path(), "react", target.path()).unwrap();
        assert!(node_modules.path().join("react").is_symlink(), "forced link should replace the unmanaged directory");
    }

    #[test]
    fn remove_obsolete_root_links_only_touches_managed_symlinks() {
        let node_modules = tempfile::tempdir().unwrap();
        let store_root = tempfile::tempdir().unwrap();
        let store = Store::for_test(store_root.path());
        let linker = Linker { store: &store, node_modules: node_modules.path().to_path_buf(), force: false };

        // A managed link to a package no longer wanted (e.g. `nermo remove left-pad`).
        let target = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(target.path(), node_modules.path().join("left-pad")).unwrap();
        // A managed scoped link, also no longer wanted.
        fs::create_dir_all(node_modules.path().join("@types")).unwrap();
        std::os::unix::fs::symlink(target.path(), node_modules.path().join("@types/left-pad")).unwrap();
        // An unmanaged real directory a user created by hand.
        fs::create_dir_all(node_modules.path().join("hand-rolled")).unwrap();
        fs::write(node_modules.path().join("hand-rolled/index.js"), "keep me").unwrap();

        linker.remove_obsolete_root_links(&BTreeSet::new()).unwrap();

        assert!(!node_modules.path().join("left-pad").exists(), "obsolete managed link should be removed");
        assert!(!node_modules.path().join("@types/left-pad").exists(), "obsolete scoped link should be removed");
        assert!(!node_modules.path().join("@types").exists(), "emptied scope dir should be cleaned up");
        assert!(node_modules.path().join("hand-rolled/index.js").is_file(), "unmanaged directory must survive");
    }
}
