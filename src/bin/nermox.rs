//! `nermox <package>[@version] [args...]`: fetch a package (and its
//! dependencies) straight from the registry and run its bin, without ever
//! touching the current project's package.json/node_modules — the same job
//! `npx`/`bunx` do. Shares nermo's global content-addressed store, so a
//! second run of the same resolved version is a cache hit (no download, no
//! re-linking), same as a real `nermo install` of an already-fetched graph.

use anyhow::{anyhow, bail, Context, Result};
use nermo::linker::Linker;
use nermo::registry;
use nermo::resolver::{PackageKey, ResolvedPackage, Resolver};
use nermo::store::Store;
use std::collections::BTreeMap;
use std::env;

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let Some((spec, script_args)) = args.split_first() else {
        eprintln!("usage: nermox <package>[@version] [args...]");
        std::process::exit(1);
    };

    match run(spec, script_args) {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("Error: {e:#}");
            std::process::exit(1);
        }
    }
}

fn run(spec: &str, args: &[String]) -> Result<i32> {
    // A bare "cowsay" has no version pin — resolve against "*" (highest
    // published), same as `nermo add`'s own no-version case.
    let (name, range) = match spec.rsplit_once('@').filter(|(n, _)| !n.is_empty()) {
        Some((n, r)) => (n.to_string(), r.to_string()),
        None => (spec.to_string(), "*".to_string()),
    };

    let client = registry::Client::new()?;
    let store = Store::open()?;

    let direct = BTreeMap::from([(name.clone(), range)]);
    let graph = Resolver::new(&client)
        .with_disk_cache(store.root().join("cache").join("registry"))
        .resolve(&direct)
        .with_context(|| format!("resolving {spec}"))?;

    let root_key: &PackageKey =
        &graph.roots.first().ok_or_else(|| anyhow!("failed to resolve {spec}"))?.key;
    let (resolved_name, resolved_version) = root_key.clone();

    let packages: Vec<(PackageKey, registry::Dist)> = graph
        .packages
        .iter()
        .map(|(key, pkg)| {
            let dist = registry::Dist { tarball: pkg.tarball.clone(), integrity: pkg.integrity.clone(), shasum: pkg.shasum.clone() };
            (key.clone(), dist)
        })
        .collect();
    store.ensure_all(&client, &packages, 16).context("downloading resolved packages")?;

    // Cached by exact resolved version, same idea as npx/bunx's own run
    // cache: a second `nermox cowsay` (same version) skips straight to
    // finding the bin, no re-resolve, re-download, or re-link.
    let cache_root = store.root().join("nermox").join(resolved_name.replace('/', "+")).join(&resolved_version);
    let marker = cache_root.join(".nermox-linked");
    if !marker.is_file() {
        Linker::new(&store, &cache_root, false).link(&graph).with_context(|| format!("linking {spec}"))?;
        std::fs::create_dir_all(&cache_root).ok();
        std::fs::write(&marker, "").context("writing nermox cache marker")?;
    }

    let pkg = &graph.packages[root_key];
    let bin_name = pick_bin(&resolved_name, pkg)?;
    let bin_dir = cache_root.join("node_modules").join(".bin");
    let bin_path = bin_dir.join(&bin_name);

    let path_var = env::var_os("PATH").unwrap_or_default();
    let new_path = env::join_paths(std::iter::once(bin_dir).chain(env::split_paths(&path_var))).context("building PATH")?;

    let status = std::process::Command::new(&bin_path)
        .args(args)
        .env("PATH", new_path)
        .status()
        .with_context(|| format!("running {}", bin_path.display()))?;
    Ok(status.code().unwrap_or(1))
}

/// Which of a package's `bin` entries to run when the caller didn't say:
/// prefer the entry matching the package's own name (npx/bunx's own
/// default), fall back to the only entry if there's exactly one, otherwise
/// this needs disambiguation nermox doesn't support yet.
fn pick_bin(pkg_name: &str, pkg: &ResolvedPackage) -> Result<String> {
    if pkg.bin.is_empty() {
        bail!("{pkg_name} declares no executable (\"bin\") entries");
    }
    let own_name = pkg_name.rsplit('/').next().unwrap_or(pkg_name);
    if pkg.bin.contains_key(own_name) {
        return Ok(own_name.to_string());
    }
    if pkg.bin.len() == 1 {
        return Ok(pkg.bin.keys().next().expect("checked non-empty above").clone());
    }
    let names: Vec<&str> = pkg.bin.keys().map(String::as_str).collect();
    bail!("{pkg_name} has multiple executables ({}); nermox doesn't support picking one yet", names.join(", "))
}
