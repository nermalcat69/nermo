# Performance report — Phase 8

Measured on macOS (Darwin 25.6.0), Node v24.13.0, npm 11.6.2, over a home network
connection to the public npm registry. Numbers are single runs, not medians —
good enough to validate direction, not to publish as guaranteed targets (per
PRD §15.4, no claim here should be read as a committed benchmark).

## What changed this phase

Store downloads (`nermo install`'s "Checking global store..." phase) now run
concurrently instead of one package at a time (`Store::ensure_all`, built on
a small thread-pool primitive in `src/concurrency.rs`). Concurrency is
bounded by `NERMO_CONCURRENCY` (default 16), matching the PRD's configuration
schema (§17.1/§17.3). `nermo install` now also prints a phase timing
breakdown (resolve / store / link / total), which is what surfaced the
finding below.

## Reproducing these numbers

```bash
export NERMO_STORE=/tmp/nermo-bench-store
mkdir /tmp/nermo-bench-nermo && cd /tmp/nermo-bench-nermo
echo '{"name":"bench","version":"1.0.0","dependencies":{"yargs":"^17.7.2"}}' > package.json

rm -rf "$NERMO_STORE"
nermo install                 # cold
rm .nermo-lock && nermo install  # cold resolve, warm store
nermo install                 # fully warm

# npm comparison, same manifest, separate directory
npm cache clean --force
npm install --no-audit --no-fund   # cold
rm -rf node_modules package-lock.json
npm install --no-audit --no-fund   # warm
```

## Results (16-package graph: `yargs@^17.7.2` and its transitive deps)

| Scenario | nermo | npm | Notes |
|---|---|---|---|
| Cold (empty store/cache) | 3.90s | 2.13s | resolve 2.37s, store 1.45s, link 0.07s |
| Fully warm (unchanged lockfile) | **0.02s** | 0.21s | resolve 0.00s, store 0.00s, link 0.00s |
| New project, shared store, no lockfile yet | 2.19s | — | store 0.00s (0 downloads); resolve 2.11s dominates |

**Warm installs are the win**: ~10x faster than npm, because a matching
lockfile skips resolution *and* the store step is a marker-file check, no
network at all. This is exactly the case the PRD prioritizes (§15.1: "most
important performance objective is minimizing the time required to install
dependencies when the global store is already populated").

**Cold installs are currently slower than npm**, and the third row shows
why: even with a fully warm *store* (zero downloads), a brand-new project
still pays 2.11s in "resolve," because `Resolver::metadata()` fetches each
distinct package's registry document one at a time (`src/resolver.rs`). The
store step got parallel downloads this phase; resolution didn't.

## Next lever (not done this phase)

Parallelize metadata fetching in the resolver, most naturally by resolving
breadth-first (one registry round trip per graph *depth*, concurrently
within a depth, instead of one round trip per *package* in DFS order), plus
a persistent on-disk metadata cache under `store/cache/registry/` so a
second project resolving the same package doesn't refetch its registry
document from scratch (PRD §8.2, §10.4). Skipped now because it's a real
restructuring of `Resolver::resolve_one`'s recursion, not a drop-in change
like the download concurrency was — worth doing once the resolver also needs
to support more of npm's range grammar (hyphen ranges, dist-tags), since
that work touches the same function.

## Known resolver gap hit while benchmarking

`express@^4.19.0` failed to resolve: one of its transitive deps
(`safer-buffer`) uses the range `">= 2.1.2 < 3"`, a space-separated
comparator set that `semver::VersionReq` doesn't parse. This is the
documented MVP limitation from `src/resolver.rs` (hyphen/OR ranges and
dist-tags aren't supported yet) — not a regression, just a real-world
package that exercises it.

## Update — Phase 9: optionalDependencies made resolution slower

Phase 9 added `optionalDependencies` resolution (needed for real frontend
tooling — see `docs/compatibility.md`). It costs real time: resolving a
fresh Vite + React scaffold (`npm create vite -- --template react`) went
from 16.6s to 43.2s for the resolve phase alone, because `rolldown` lists
12+ per-platform native-binary packages as optional dependencies, and the
resolver fetches each one's registry metadata just to discard 11 of them
based on their `os`/`cpu` fields (`Resolver::resolve_optional` in
`src/resolver.rs`). This makes the "parallelize/cache metadata fetches"
lever above measurably more valuable than it was last phase — most of that
43s is now provably wasted network time, not resolution logic.

## Update — resolver concurrency (the lever from the last two sections)

Implemented: `Resolver` now fans out each node's dependency edges
concurrently instead of one registry round trip at a time, depth-first
(`Resolver::fan_out` in `src/resolver.rs`, built on the same
`concurrency::parallel_for_each` primitive the store already used for
downloads). Metadata cache and the in-progress package map moved from
plain `HashMap`/`BTreeMap` behind `&mut self` to `Mutex`-wrapped fields so
sibling threads can share them safely; cycle-safety (a package claims its
graph slot under one lock acquisition before recursing) works the same as
before, just now also race-safe across threads. Persistent on-disk metadata
caching (the other half of the originally-flagged lever) is still not
done — this only addresses the "parallelize" half.

`nermo install` output now also formats every duration as milliseconds
below 1 second and seconds above it (`format_duration` in `src/main.rs`),
matching how `format_bytes` already scales disk sizes — this is what makes
the near-instant warm-install numbers below legible instead of showing
`0.00s` for everything under a second.

### Before / after, same scenarios as above

| Scenario | Resolve time before | Resolve time after | Speedup |
|---|---|---|---|
| `yargs@^17.7.2`, cold store | 2.37s | **1.12s** | ~2.1x |
| `yargs@^17.7.2`, new project, warm store | 2.11s | **1.05s** | ~2.0x |
| Vite + React scaffold (25 packages incl. rolldown's optional native binary) | 43.19s | **18.60s** | ~2.3x |

Full `nermo install` output, cold, on the `yargs` project:

```
Resolving dependencies...
Found 16 packages.

Checking global store...
Downloaded: 16, reused: 0

Linking dependencies...

Installation completed.

Packages: 16
Downloaded: 16
Reused: 0
Duration: 2.58s (resolve 1.12s, store 1.39s, link 55ms)
```

...and fully warm, right after:

```
Duration: 4ms (resolve 0ms, store 0ms, link 1ms)
```

Correctness was re-verified after this change, not just speed: the Vite +
React project still `vite build`s successfully from the concurrently-resolved
graph, a second `install` on the same project is a full lockfile-driven cache
hit (`Downloaded: 0, reused: 25`), and all 26 unit tests still pass.

### Why only ~2x, not closer to `CONCURRENCY` (16x)

Fan-out width is bounded by how many *sibling* dependencies a single package
declares, not by the total graph size — `yargs` itself has around 9 direct
dependencies, most of which are near-leaves with 0-2 dependencies of their
own, so most fan-out batches in this graph are small. The graph's *depth*
(sequential levels — a child's metadata fetch can't start until its parent's
version is picked) is unavoidably serial and still gates the total. Real
speedup scales with how wide the graph is at each level, not with
`CONCURRENCY` itself — the Vite/rolldown case saw a bigger win specifically
because it has a genuinely wide fan-out (12+ sibling optional-dependency
variants at one level).

### Next lever (still not done)

Persistent on-disk metadata caching (`store/cache/registry/`, PRD §8.2/§10.4)
would help the *cross-project* and *cross-run* case specifically — e.g. the
second `yargs` project above still pays a full 1.05s resolving packages the
first project resolved moments earlier, because nothing survives between
separate `nermo install` invocations. Unlike the concurrency fix, caching
wouldn't help a single cold resolve of a brand-new package tree (there's
nothing to have cached yet), so the two levers are complementary, not
alternatives.

## Update — a real 484-package production project, and two timeout bugs

A real project (React Router + Cloudflare Workers + Drizzle + Radix UI + ~50
direct dependencies) surfaced two genuine bugs that none of the synthetic
benchmarks above caught, both around registry request timeouts, plus a
missing resolver feature (npm dependency aliases — see
`docs/compatibility.md`; unrelated to performance but hit in the same run).

**Bug 1: `@types/node`'s full metadata document is 11MB.** Fixed by
requesting npm's "abbreviated" install-metadata format
(`Accept: application/vnd.npm.install-v1+json`), the same one npm/pnpm
themselves use — cuts it to ~2MB for `@types/node` while keeping every field
the resolver needs.

**Bug 2: that fix wasn't enough for every package.** `wrangler` has ~700
published versions each listing dozens of dependencies; its abbreviated
metadata is still ~16MB, because the bloat here is in dependency data the
resolver actually needs, not the readmes/history the abbreviated format
strips. No metadata-format trick shrinks a payload that's legitimately that
large. The real bug was the flat 30-second request timeout (tuned against a
fast connection and small packages) applying to *every* registry request,
including tarball downloads for large native packages. Fixed by raising it
to 180s with a separate 10s `connect_timeout` — a real timeout should only
fire when nothing is happening, not when a large-but-progressing transfer is
merely slow.

Full install of the real project, cold (empty store, no lockfile):

```
Found 484 packages.
Multiple versions in the graph: (20 packages, e.g. wrangler: 4.141.0, 4.142.0)
Checking global store...
Downloaded: 484, reused: 0
Duration: 129.47s (resolve 50.91s, store 72.25s, link 6.29s)
```

Second install right after (unchanged lockfile, warm store):

```
Packages: 484
Downloaded: 0
Reused: 484
Duration: 64ms (resolve 1ms, store 1ms, link 54ms)
```

129s → 64ms, ~2000x, for the case the PRD says matters most (§15.1: warm
installs). The cold number is honest, not great — 484 packages is a large
graph, and the still-sequential-across-nodes resolver depth (not the
per-node fan-out width) plus 484 tarball downloads both take real time. This
is real production data, not a synthetic benchmark, and it's the best
evidence yet that the still-undone "parallelize/cache resolver metadata
across the whole graph, not just per-node" lever matters at real-world scale.
