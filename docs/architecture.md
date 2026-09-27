# Architecture

Single crate (`src/main.rs` + modules), not the multi-crate workspace sketched
in the PRD's §23.3 — deliberately: the PRD itself says a single crate is more
practical for the first prototype, and nothing here has outgrown one yet.

```
manifest   -- reads package.json, walks up to find the project root
registry   -- npm-registry HTTP client (metadata + tarball download)
archive    -- tarball integrity verification + safe extraction
store      -- the global, content-shared package store (~/.local/share/nermo/store etc.)
resolver   -- package.json + registry metadata -> resolved dependency graph
lockfile   -- .nermo-lock read/write, manifest-drift detection
linker     -- resolved graph -> a real node_modules
concurrency-- generic bounded-thread-pool helper used by store downloads
```

## Data flow (`nermo install`)

```
package.json ──> manifest::load
                     │
                     ▼
      .nermo-lock present & matches package.json?
        │ yes                              │ no
        ▼                                  ▼
 lockfile::to_graph()          resolver::Resolver::resolve()
   (no network)                  (registry metadata, semver matching)
        │                                  │
        └────────────────┬─────────────────┘
                          ▼
                 store::Store::ensure_all()
             (parallel download+verify+extract
              of anything not already cached)
                          │
                          ▼
                 linker::Linker::link()
          (node_modules/.nermo/<name>@<version>/
           virtual-store entries + symlink farm)
                          │
                          ▼
              lockfile::Lockfile::save() (if freshly resolved)
              store::Store::track_project() (for `store prune`)
```

## Why the linker hardlinks instead of symlinking whole packages

The obvious approach — `node_modules/react -> store/packages/react/19.0.0`
— is wrong. Node resolves a symlinked file to its real path *before*
computing where to search for that module's own `node_modules`, so anything
`react` itself `require()`s would search inside the immutable, dependency-free
store instead of the project. `linker.rs` instead builds a private "virtual
store" per resolved `(name, version)` under `node_modules/.nermo/`,
populating it with hardlinks (which have no separate "real" location, unlike
symlinks) of the package's own files, plus its own `node_modules` of
*directory* symlinks to its dependencies' virtual entries. This is the same
scheme pnpm uses, and the PRD names pnpm's store/symlink layout as a direct
inspiration (§1.4).

Store files are locked read-only (0o444) at commit time specifically so
hardlinking them into multiple projects can't let one project's build tool
corrupt another project's shared dependency (see §6.2/§16.5 in the PRD).

## Concurrency model

Everything here is synchronous/blocking (`reqwest::blocking`), not
async/tokio — deliberately, since the PRD's own recommended-libraries list
names Tokio but the actual workload (a CLI that runs, does I/O, and exits) is
latency-bound, not concurrency-bound in a way that needs an async runtime.
Where concurrency does help (parallel package downloads in
`Store::ensure_all`), it's plain OS threads via `std::thread::scope` and the
small `concurrency::parallel_for_each` helper — no new dependency, no
executor to configure. See `docs/performance.md` for measurements and where
this does *not* yet help (resolver metadata fetches are still sequential).

## What's intentionally not here yet

See `docs/compatibility.md` for the full list of known gaps (workspaces,
lifecycle scripts, `.bin` shims, Windows junctions, native-package platform
selection edge cases, and the resolver's version-range subset).
