# Compatibility

## Platforms

| Platform | Status |
|---|---|
| macOS | Tested throughout development (this machine): install, store, prune, linking, and a real Vite + React build all verified working. |
| Linux | Not run on real Linux. The code paths that differ (XDG store paths, Unix symlinks, `chmod`-based store immutability) are all behind `cfg!(target_os = ...)`/`#[cfg(unix)]` and share the same code as macOS (both are `unix`), so they're expected to work, but "expected" isn't "tested." |
| Windows | Not run. Known gap: `linker::symlink_dir` calls `std::os::windows::fs::symlink_dir` directly, which needs Developer Mode or admin rights on stock Windows. The PRD's suggested junction-based fallback (§11.6) isn't implemented. The store's read-only permission lock (`store::lock_permissions`) is also a no-op on non-Unix — nothing currently protects Windows store contents from accidental modification via a hardlink. |

## What's been validated against a real project

`npm create vite@latest -- --template react`'s dependencies were installed
entirely with `nermo install`, and `vite build` (invoked directly, see
below) produced a working production bundle from the nermo-linked
`node_modules`. This is a real, non-trivial test: it exercises ESM resolution,
a native binary (`rolldown`'s platform-specific binding), and nested
`node_modules` resolution through the linker's virtual store.

## Known gaps

- **`.bin` shims aren't created.** `node_modules/.bin/vite` doesn't exist,
  so `npm run build` / `npm run dev` (which rely on `.bin` being on `PATH`)
  won't find the binary. Workaround used during testing: invoke the
  package's entry script directly (`node node_modules/vite/bin/vite.js
  build`). This is a real usability gap, not just a missing nicety — most
  projects invoke tools through npm scripts.
- **Resolver only understands a subset of npm's version-range grammar.**
  Exact versions, `^`, `~`, comparator operators, and `*` work (anything
  `semver::VersionReq` parses). Not supported: space-separated comparator
  sets (`">= 2.1.2 < 3"`, which real packages use — `express`'s
  `safer-buffer` dependency hit this during benchmarking), hyphen ranges
  (`"1.2.3 - 2.3.4"`), OR ranges (`"1.x || 2.x"`), and dist-tags like
  `"latest"`. See `src/resolver.rs::pick_version`.
- **Lifecycle scripts (`postinstall` etc.) are never executed.** This is a
  deliberate, documented policy (PRD §16.3, §30: "disabled by default"), not
  an oversight — but it means packages that rely on a build step at install
  time (native addons compiled via `node-gyp`, for example) won't work.
  There's no flag yet to opt back in.
- **`optionalDependencies` are resolved but not cheaply.** Support was added
  in Phase 9 specifically so native-binary-distributing packages (esbuild,
  swc, sharp, rolldown, ...) work at all — see `docs/architecture.md` and
  the Phase 9 note in `docs/performance.md`. The cost: every platform
  variant listed gets a registry metadata fetch just to be filtered out by
  its `os`/`cpu` fields, since npm doesn't expose "give me only the variant
  for platform X" as a single request.
- **`.nermo-lock` isn't cross-platform.** See `docs/lockfile.md` — a
  resolved `optionalDependencies` entry (a specific platform's native
  binary) is baked into the lockfile as-is. Don't share a committed
  lockfile across an OS/CPU boundary yet.
- **Single-package projects only.** No workspace/monorepo support
  (PRD §19) — `manifest::find_project_root` finds the nearest `package.json`
  and that's the whole project as far as nermo is concerned.
- **No native-package ABI matching beyond `os`/`cpu`.** npm's own
  `optionalDependencies` platform filtering doesn't check the Node.js ABI
  version a native binding was built against (PRD §20.4) — nermo doesn't
  either. In practice this hasn't caused a failure yet, but it's not solved.
- **Symlinks inside a downloaded tarball are refused outright**
  (`archive::extract`), not recreated. npm strips symlinks from published
  packages already, so this hasn't been observed to matter in practice; it's
  a deliberate security choice (see the security note below) rather than a
  missing feature.

## Security posture (relevant to compatibility because it affects what extracts)

- Every downloaded tarball is verified against the registry's declared
  `integrity` (SHA-512) or, for older packages, `shasum` (SHA-1) before
  extraction. A mismatch is a hard failure — nothing partially-verified ever
  reaches the store.
- Extraction rejects any archive entry whose path would escape the
  destination directory (`archive::safe_join`), and rejects symlink/hardlink
  entries entirely rather than trying to validate an escape-prone link
  target (see the tests in `src/archive.rs`).
- Store contents are `chmod`'d read-only (`0o444`) on Unix right after
  extraction, before anything else can hardlink them into a project — this
  is what makes hardlinking (used by the linker) safe against one project's
  build tool corrupting another project's copy of a shared dependency.
