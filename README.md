# nermo

A local-first JavaScript package manager, written in Rust, that shares one
global package store across every project on your machine instead of
duplicating `node_modules` everywhere.

## Install

**Download a release** (no Rust toolchain needed): grab the archive for your
platform from the [Releases page](../../releases/latest), extract it, and put
the `nermo` binary somewhere on your `PATH`:

```bash
tar xzf nermo-*-aarch64-apple-darwin.tar.gz   # or the archive matching your platform
mv nermo-*/nermo /usr/local/bin/
nermo --version
```

**Or build from source:**

```bash
cargo build --release
./target/release/nermo --help
```

Every push of a `v*.*.*` tag triggers `.github/workflows/release.yml`, which
builds binaries for macOS (Intel + Apple Silicon), Linux, and Windows and
attaches them to a GitHub Release automatically.

## Usage

```bash
cd your-project         # any project with a package.json
nermo install            # resolve, download into the shared store, link node_modules
nermo install --frozen   # require .nermo-lock to be present and up to date; never re-resolve
nermo install --prune    # after installing, also free any store package no project uses anymore
nermo remove react zod   # remove dependencies from package.json and reinstall to match
nermo fetch react@19.0.0 # ensure one package version is in the store, without installing a project
nermo store               # show store stats (packages, disk usage, tracked projects)
nermo store prune         # remove packages no tracked project references (supports --dry-run, --yes)
nermo doctor              # diagnose store/registry/project/lockfile/symlink problems
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
store management (`store`, `store prune`, opt-in auto-prune on install),
`remove`, `doctor`, and concurrent downloads/resolution. Not yet implemented:
`--offline`, workspaces, `.bin` shims, and lifecycle scripts (disabled by
design, not by accident — see `docs/compatibility.md`).

CI (`.github/workflows/ci.yml`) builds and runs the full test suite on
macOS, Linux, and Windows for every push and pull request.
