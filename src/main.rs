mod archive;
mod concurrency;
mod linker;
mod lockfile;
mod manifest;
mod progress;
mod registry;
mod resolver;
mod selfupdate;
mod store;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::env;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Parser)]
#[command(name = "nermo", version, about = "Local-first JS package manager")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Discover the project and show what would be installed.
    Install {
        /// Require a compatible, up-to-date lockfile; never re-resolve.
        #[arg(long)]
        frozen: bool,
        /// After installing, remove any store package this project no
        /// longer depends on if no other tracked project needs it either
        /// (e.g. an old `next` version left behind after an upgrade).
        #[arg(long)]
        prune: bool,
        /// Overwrite unmanaged node_modules entries left by another package
        /// manager (e.g. after switching from bun/npm/pnpm) instead of
        /// refusing to touch them. Off by default: this deletes files nermo
        /// didn't create.
        #[arg(long)]
        force: bool,
    },
    /// Ensure a package version is present in the global store, downloading
    /// it only if it isn't already cached (Phase 2 registry + Phase 3 store).
    Fetch {
        /// e.g. "react@19.0.0" or "@types/node@22.0.0"
        spec: String,
    },
    /// Remove one or more dependencies from package.json, then reinstall to
    /// update the lockfile and node_modules to match.
    Remove {
        #[arg(required = true)]
        names: Vec<String>,
    },
    /// Inspect or maintain the global package store.
    Store {
        #[command(subcommand)]
        action: Option<StoreCommand>,
    },
    /// Diagnose common configuration and installation problems.
    Doctor,
    /// Download and install the latest nermo release in place of the
    /// running binary.
    Upgrade,
}

#[derive(Subcommand)]
enum StoreCommand {
    /// Remove packages not referenced by any tracked project.
    Prune {
        /// Show what would be removed without removing it.
        #[arg(long)]
        dry_run: bool,
        /// Skip the confirmation prompt.
        #[arg(long, short = 'y')]
        yes: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let is_upgrade = matches!(cli.command, Command::Upgrade);

    let result = match cli.command {
        Command::Install { frozen, prune, force } => install(frozen, prune, force),
        Command::Fetch { spec } => fetch(&spec),
        Command::Remove { names } => remove(&names),
        Command::Store { action: None } => store_stats(),
        Command::Store { action: Some(StoreCommand::Prune { dry_run, yes }) } => store_prune(dry_run, yes),
        Command::Doctor => doctor(),
        Command::Upgrade => selfupdate::upgrade(),
    };

    // Runs on every command except `upgrade` itself: a cheap, cached,
    // best-effort check (see selfupdate::notify_if_update_available) so a
    // newer release surfaces on its own instead of requiring the user to
    // remember to check. Never affects this command's own exit code.
    if !is_upgrade {
        selfupdate::notify_if_update_available();
    }

    result
}

fn fetch(spec: &str) -> Result<()> {
    let (name, version) = spec
        .rsplit_once('@')
        .filter(|(n, _)| !n.is_empty())
        .ok_or_else(|| anyhow::anyhow!("expected \"name@version\", got {spec}"))?;

    let client = registry::Client::new()?;
    let store = store::Store::open()?;
    let (path, reused) = store.ensure(&client, name, version, || Ok(client.version_metadata(name, version)?.dist))?;

    if reused {
        println!("Already in store: {}", path.display());
    } else {
        println!("Downloaded and stored {name}@{version} at {}", path.display());
    }
    Ok(())
}

fn remove(names: &[String]) -> Result<()> {
    let cwd = env::current_dir()?;
    let root = manifest::find_project_root(&cwd)?;

    let removed = manifest::remove_dependencies(&root, names)?;
    for name in names {
        if removed.contains(name) {
            println!("Removed {name}");
        } else {
            println!("{name} was not a dependency; skipped");
        }
    }
    if removed.is_empty() {
        return Ok(());
    }

    // package.json changed, so the existing lockfile won't match and
    // install() will naturally re-resolve, relink (dropping the removed
    // package's now-obsolete node_modules entry), and rewrite the lockfile.
    println!();
    install(false, false, false)
}

fn install(frozen: bool, prune: bool, force: bool) -> Result<()> {
    let install_start = Instant::now();
    let cwd = env::current_dir()?;
    let root = manifest::find_project_root(&cwd)?;
    let manifest = manifest::load(&root)?;

    println!("Project: {}", manifest.name.as_deref().unwrap_or("(unnamed)"));
    println!("Root: {}", root.display());

    let mut direct = manifest.dependencies.clone();
    direct.extend(manifest.dev_dependencies.clone());
    if direct.is_empty() {
        println!("No dependencies declared.");
        return Ok(());
    }

    let client = registry::Client::new()?;
    let store = store::Store::open()?;
    let existing_lock = lockfile::Lockfile::load(&root)?;
    let reusable_lock = existing_lock.as_ref().filter(|lock| lock.matches_manifest(&direct));

    let resolve_start = Instant::now();
    let (graph, lock_to_write) = match reusable_lock {
        Some(lock) => {
            println!("\nUsing existing lockfile (package.json unchanged).");
            (lock.to_graph()?, None)
        }
        None if frozen => {
            if existing_lock.is_some() {
                anyhow::bail!("--frozen requires the lockfile, but package.json has changed since it was written");
            }
            anyhow::bail!("--frozen requires an existing {}, but none was found", lockfile::FILE_NAME);
        }
        None => {
            if existing_lock.is_some() {
                println!("\npackage.json has changed since the lockfile was written; not using it.");
            }
            // The disk cache means a second project resolving a package
            // another project already resolved recently skips the registry
            // round trip entirely, not just the store/download step.
            let spinner = progress::Spinner::start("Resolving dependencies");
            let graph = resolver::Resolver::new(&client)
                .with_disk_cache(store.root().join("cache").join("registry"))
                .resolve(&direct);
            spinner.stop();
            let graph = graph?;
            let lock = lockfile::Lockfile::from_graph(&direct, &graph);
            (graph, Some(lock))
        }
    };
    let resolve_elapsed = resolve_start.elapsed();
    println!("Found {} packages.", graph.packages.len());

    let mut versions_per_name: std::collections::BTreeMap<&str, Vec<&str>> = std::collections::BTreeMap::new();
    for (name, version) in graph.packages.keys() {
        versions_per_name.entry(name).or_default().push(version);
    }
    let conflicts: Vec<_> = versions_per_name.iter().filter(|(_, vs)| vs.len() > 1).collect();
    if !conflicts.is_empty() {
        println!("Multiple versions in the graph:");
        for (name, versions) in conflicts {
            println!("  {name}: {}", versions.join(", "));
        }
    }

    // Every package here was just resolved, so its tarball URL and integrity
    // hash are already known — passing them along means ensure_all makes
    // zero metadata requests, only tarball downloads for what's missing.
    let packages: Vec<_> = graph
        .packages
        .iter()
        .map(|(key, pkg)| {
            let dist = registry::Dist {
                tarball: pkg.tarball.clone(),
                integrity: pkg.integrity.clone(),
                shasum: pkg.shasum.clone(),
            };
            (key.clone(), dist)
        })
        .collect();
    let store_start = Instant::now();
    let spinner = progress::Spinner::start("Checking global store");
    let downloaded = store.ensure_all(&client, &packages, install_concurrency());
    spinner.stop();
    let downloaded = downloaded?;
    let store_elapsed = store_start.elapsed();
    println!("Downloaded: {downloaded}, reused: {}", packages.len() - downloaded);

    let link_start = Instant::now();
    let spinner = progress::Spinner::start("Linking dependencies");
    let link_result = linker::Linker::new(&store, &root, force).link(&graph);
    spinner.stop();
    link_result?;
    let link_elapsed = link_start.elapsed();

    if let Some(lock) = lock_to_write {
        lock.save(&root)?;
    }
    store.track_project(&root, graph.packages.keys().cloned())?;

    if prune {
        // Recomputed *after* this project's own tracked entry was updated
        // above, so a version this project just dropped (e.g. an old `next`
        // it upgraded away from) is already excluded from "referenced" if
        // this was the last project still using it.
        let mut unused = store.unused_packages()?;
        unused.sort();
        if unused.is_empty() {
            println!("\nNo unused packages to prune.");
        } else {
            let mut freed = 0u64;
            for (name, version) in &unused {
                freed += store.remove_package(name, version)?;
            }
            println!("\nPruned {} unused package(s), freed {}:", unused.len(), format_bytes(freed));
            for (name, version) in &unused {
                println!("  {name}@{version}");
            }
        }
    }

    println!("\nInstallation completed.\n");
    println!("Packages: {}", graph.packages.len());
    println!("Downloaded: {downloaded}");
    println!("Reused: {}", packages.len() - downloaded);
    println!(
        "Duration: {} (resolve {}, store {}, link {})",
        format_duration(install_start.elapsed()),
        format_duration(resolve_elapsed),
        format_duration(store_elapsed),
        format_duration(link_elapsed),
    );
    Ok(())
}

/// Human-scaled duration: milliseconds below one second, seconds (2 decimal
/// places) at or above it — matches `format_bytes`'s "smallest readable
/// unit" approach instead of always printing fractional seconds.
fn format_duration(d: std::time::Duration) -> String {
    let ms = d.as_secs_f64() * 1000.0;
    if ms < 1000.0 {
        format!("{ms:.0}ms")
    } else {
        format!("{:.2}s", d.as_secs_f64())
    }
}

/// Bounded thread count for concurrent downloads (PRD §17.1/§17.3).
fn install_concurrency() -> usize {
    env::var("NERMO_CONCURRENCY").ok().and_then(|s| s.parse().ok()).filter(|&n| n > 0).unwrap_or(16)
}

fn store_stats() -> Result<()> {
    let store = store::Store::open()?;
    let stats = store.stats()?;

    println!("Nermo Store\n");
    println!("Location:\n{}\n", stats.location.display());
    println!("Packages:          {}", stats.package_names);
    println!("Package versions:  {}", stats.package_versions);
    println!("Disk usage:        {}", format_bytes(stats.disk_usage_bytes));
    println!("\nProjects tracked:  {}", stats.projects_tracked);
    Ok(())
}

fn store_prune(dry_run: bool, yes: bool) -> Result<()> {
    let store = store::Store::open()?;
    let mut unused = store.unused_packages()?;
    unused.sort();

    println!("Nermo Store Cleanup\n");
    if unused.is_empty() {
        println!("No unused packages found.");
        return Ok(());
    }

    let mut total_bytes = 0u64;
    for (name, version) in &unused {
        total_bytes += store.package_disk_usage(name, version).unwrap_or(0);
    }

    println!("Unused packages: {}", unused.len());
    println!("Potential space recovery: {}\n", format_bytes(total_bytes));
    for (name, version) in &unused {
        println!("  {name}@{version}");
    }

    if dry_run {
        println!("\n(dry run — nothing removed)");
        return Ok(());
    }

    if !yes {
        print!("\nProceed? [y/N] ");
        std::io::Write::flush(&mut std::io::stdout())?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Aborted.");
            return Ok(());
        }
    }

    let mut freed = 0u64;
    for (name, version) in &unused {
        freed += store.remove_package(name, version)?;
    }
    println!("\nRemoved {} packages, freed {}.", unused.len(), format_bytes(freed));
    Ok(())
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

/// Diagnose common problems (PRD §13.9): store health, registry reachability,
/// and — when run inside a project — manifest/lockfile consistency and
/// broken symlinks in node_modules. Never modifies anything; every check
/// reports ok/warn/fail independently so one failure doesn't hide the rest.
fn doctor() -> Result<()> {
    println!("Nermo Doctor\n");
    let mut problems = 0usize;

    println!("[ok]   nermo {} running", env!("CARGO_PKG_VERSION"));

    match store::Store::open() {
        Ok(store) => {
            let probe = store.root().join(".doctor-write-test");
            match std::fs::write(&probe, b"x").and_then(|_| std::fs::remove_file(&probe)) {
                Ok(()) => println!("[ok]   store is accessible and writable ({})", store.root().display()),
                Err(e) => {
                    println!("[fail] store at {} is not writable: {e}", store.root().display());
                    problems += 1;
                }
            }
        }
        Err(e) => {
            println!("[fail] could not open the store: {e}");
            problems += 1;
        }
    }

    match registry::Client::new().and_then(|c| c.check_connectivity()) {
        Ok(()) => println!("[ok]   registry reachable (https://registry.npmjs.org)"),
        Err(e) => {
            println!("[fail] registry unreachable: {e}");
            problems += 1;
        }
    }

    match manifest::find_project_root(&env::current_dir()?) {
        Err(_) => println!("\n(not inside a project — skipping project-specific checks)"),
        Ok(root) => {
            println!("\nProject: {}", root.display());

            let manifest = match manifest::load(&root) {
                Ok(m) => {
                    println!("[ok]   package.json parses");
                    Some(m)
                }
                Err(e) => {
                    println!("[fail] package.json: {e}");
                    problems += 1;
                    None
                }
            };

            match (lockfile::Lockfile::load(&root), &manifest) {
                (Ok(Some(lock)), Some(manifest)) => {
                    let mut direct = manifest.dependencies.clone();
                    direct.extend(manifest.dev_dependencies.clone());
                    if lock.matches_manifest(&direct) {
                        println!("[ok]   {} is up to date with package.json", lockfile::FILE_NAME);
                    } else {
                        println!(
                            "[warn] {} is stale (package.json changed); run `nermo install` to refresh it",
                            lockfile::FILE_NAME
                        );
                    }
                }
                (Ok(None), _) => println!("[info] no {} yet; run `nermo install`", lockfile::FILE_NAME),
                (Err(e), _) => {
                    println!("[fail] {}: {e}", lockfile::FILE_NAME);
                    problems += 1;
                }
                (Ok(Some(_)), None) => {} // package.json already reported as broken above
            }

            let broken = find_broken_symlinks(&root.join("node_modules"))?;
            if broken.is_empty() {
                println!("[ok]   no broken symlinks in node_modules");
            } else {
                println!("[fail] {} broken symlink(s) in node_modules:", broken.len());
                for path in &broken {
                    println!("         {}", path.display());
                }
                problems += broken.len();
            }
        }
    }

    println!();
    if problems == 0 {
        println!("No problems found.");
    } else {
        println!("{problems} problem(s) found.");
    }
    Ok(())
}

/// Any symlink under `node_modules` whose target no longer exists — usually
/// means the global store was pruned or moved out from under a project that
/// hasn't been reinstalled since.
fn find_broken_symlinks(node_modules: &Path) -> Result<Vec<PathBuf>> {
    if !node_modules.is_dir() {
        return Ok(Vec::new());
    }
    let mut broken = Vec::new();
    for entry in walkdir::WalkDir::new(node_modules) {
        let entry = entry.context("walking node_modules")?;
        if entry.path_is_symlink() && !entry.path().exists() {
            broken.push(entry.path().to_path_buf());
        }
    }
    Ok(broken)
}
