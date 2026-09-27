# nermo

A local-first JavaScript package manager, written in Rust, that shares one
global package store across every project on your machine instead of
duplicating `node_modules` everywhere. See `prd.md` for the full product spec.

## Build

```bash
cargo build --release
./target/release/nermo --help
```

## Usage

```bash
cd your-project        # any project with a package.json
nermo install           # resolve, download into the shared store, link node_modules
nermo install --frozen  # require .nermo-lock to be present and up to date; never re-resolve
nermo fetch react@19.0.0 # ensure one package version is in the store, without installing a project
nermo store              # show store stats (packages, disk usage, tracked projects)
nermo store prune        # remove packages no tracked project references (supports --dry-run, --yes)
```

Projects keep an ordinary `package.json` and `node_modules` — nermo adds only
an optional `.nermo-lock` (see `docs/lockfile.md`). Nothing nermo-specific is
required to deploy: install with npm/pnpm/Bun instead at any time.

## Docs

- `docs/architecture.md` — module layout and the linker's design
- `docs/lockfile.md` — `.nermo-lock` schema
- `docs/configuration.md` — environment variables
- `docs/compatibility.md` — platforms tested, known gaps
- `docs/performance.md` — benchmarks against npm, with reproduction steps

## Status

Implements the PRD's MVP P0 feature set (resolve → store → link → lock) plus
store management (`store`, `store prune`) and concurrent downloads. Not yet
implemented: `nermo remove`, `nermo doctor`, `--offline`, workspaces, `.bin`
shims, and lifecycle scripts (disabled by design, not by accident — see
`docs/compatibility.md`).
