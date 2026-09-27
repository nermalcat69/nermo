# `.nermo-lock` format

JSON, schema version 1. Written by `nermo install` whenever it resolves
fresh (i.e. no existing lockfile matched `package.json`); read by every
`install` to decide whether resolution can be skipped, and required (and
never rewritten) when `--frozen` is passed.

```json
{
  "lockfileVersion": 1,
  "direct": {
    "debug": {
      "range": "^4.3.0",
      "resolved": "4.4.3"
    }
  },
  "packages": {
    "debug@4.4.3": {
      "version": "4.4.3",
      "resolved": "https://registry.npmjs.org/debug/-/debug-4.4.3.tgz",
      "integrity": "sha512-...",
      "shasum": null,
      "dependencies": {
        "ms": "2.1.3"
      }
    },
    "ms@2.1.3": {
      "version": "2.1.3",
      "resolved": "https://registry.npmjs.org/ms/-/ms-2.1.3.tgz",
      "integrity": "sha512-...",
      "shasum": null,
      "dependencies": {}
    }
  }
}
```

## Fields

- **`lockfileVersion`** — currently always `1`. A lockfile with any other
  value is rejected outright rather than guessed at (`Lockfile::load` in
  `src/lockfile.rs`); there's no migration logic yet because there's only
  ever been one version.
- **`direct`** — the project's own dependencies (from both `dependencies` and
  `devDependencies`) as they were when this lockfile was written: the range
  requested, and the exact version it resolved to. `install` recomputes the
  current ranges from `package.json` and compares them against this map
  (`Lockfile::matches_manifest`) — any difference (added, removed, or
  changed range) means the lockfile is stale and gets ignored (or, under
  `--frozen`, gets treated as an error instead of a silent re-resolve).
- **`packages`** — every resolved package in the graph, keyed by
  `"name@version"` (scoped packages keep their `@scope/name@version` form
  intact — the key is split on the *last* `@`, so this is unambiguous).
  `dependencies` here are that package's own direct dependency edges,
  already resolved to exact versions — this is what lets
  `Lockfile::to_graph` rebuild the full graph with zero registry calls.

## What's deliberately not in here (yet)

- **No content hash / content-addressable key.** `resolved` is a tarball
  URL, and `integrity`/`shasum` is what's verified against on download —
  there's no separate normalized-content hash (PRD §8.4's future
  content-addressable store).
- **No platform-conditional entries — this is a known correctness gap, not
  just a missing feature.** A package resolved via `optionalDependencies`
  (e.g. a native binary matching the current OS/CPU, see
  `docs/compatibility.md`) is baked into `packages` exactly like any other
  dependency, for whichever platform generated the lockfile.
  `Lockfile::to_graph` reconstructs the graph verbatim with no
  re-verification against the current machine, so committing a `.nermo-lock`
  from one platform and installing from it on another will try to fetch and
  link the *original* platform's native binary. Until the lockfile schema
  gains a platform-conditional `packages` shape, treat `.nermo-lock` as
  single-platform: don't commit and share it across an OS/CPU boundary.
