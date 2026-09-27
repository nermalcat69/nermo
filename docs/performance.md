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

## Update — tested raising concurrency on the same real project; it didn't help

The user reported the ~106s cold install (480 packages, a React Router +
Cloudflare Workers stack: esbuild, sharp, lightningcss, wrangler/workerd)
felt slow. Before touching anything, reproduced it on an isolated copy of
their real `package.json` (108.52s, matching their 106.43s) and tested
`NERMO_CONCURRENCY=48` against the default of 16:

| Concurrency | resolve | store | total |
|---|---|---|---|
| 16 (default) | 46.14s | 56.27s | 108.52s |
| 48 | 49.15s | 56.28s | 105.50s |

No measurable difference — store phase is identical to the second decimal.
**Raising concurrency was a plausible-sounding fix that turned out to be a
red herring; the number was tested, not assumed, before writing it up.**

Root cause instead: this stack is unusually heavy with native-binary
tooling. esbuild, sharp, lightningcss, and `@cloudflare/workerd` each
declare ~15-20 `optionalDependencies` (one per OS/CPU platform), and there's
no npm registry API to ask "does this match my platform?" without fetching
that candidate's full metadata first (`Resolver::resolve_optional` in
`src/resolver.rs`). Those fetches already run concurrently per node — that's
exactly why more concurrency didn't help, the graph's *width* at each node
isn't the bottleneck, the sheer *count* of necessarily-wasted round trips
is. There's no batch metadata API to eliminate this, and a name-pattern
heuristic to skip likely-wrong-platform fetches without confirming via
metadata was considered and rejected: it would trade a slow-but-correct
install for a fast-but-possibly-silently-wrong one.

What actually matters for this complaint: a second `install` of the exact
same project (same lockfile, warm store) took **102ms**. The ~106s is a
one-time cost paid when a dependency tree is first discovered fresh (new
clone, deleted lockfile, or CI with no cache) — not the steady-state cost of
working on the same checkout.

## Update — persistent registry metadata cache (closes the flagged lever)

Reviewing `bun.md` (a research document comparing Bun's install architecture)
alongside the concurrency investigation above surfaced the actual gap: the
global store already dedupes downloaded *content* perfectly across
projects — verified repeatedly (`store::tests::old_versions_become_unused_*`,
the multi-project `commander`/`next` runs in earlier sessions) — but it
didn't dedupe the *metadata lookup*. A second project resolving `react` for
the first time (no lockfile of its own yet) still paid a full registry round
trip, even though a sibling project had fetched that exact document minutes
earlier. This is the "persistent on-disk metadata cache" flagged as the next
lever in the sections above.

Implemented as a TTL-based (1 hour) cache at `store/cache/registry/`,
keyed by package name (`Resolver::with_disk_cache`, `src/resolver.rs`).
Deliberately a full skip-the-network TTL cache, not ETag conditional-GET:
ETag revalidation would still pay the full round-trip *latency* (which
dominates over payload size per the abbreviated-metadata investigation
above) just to save bandwidth on a 304 — it wouldn't actually make a second
project's resolve faster. A TTL cache trades a narrow, well-precedented risk
(a version published in the last hour might not be seen by a brand-new
resolve during that window) for eliminating the round trip entirely. It
only affects fresh resolution — a project with a matching lockfile never
calls this at all.

Verified live: two fresh projects (`a`, `b`), neither with a lockfile,
both depending on `commander@^12.0.0`, against an initially empty store:

```
project a (first ever resolve): resolve 621ms, store 601ms
project b (shares the dependency, resolved moments later): resolve 2ms, store 0ms
```

Confirmed the cache is real, not an in-memory fluke that would vanish
between separate process invocations: `store/cache/registry/commander.json`
exists on disk with the full 124-version metadata document and a
`fetched_at_unix` timestamp.

## Update — leaf-package shortcut (link-phase speedup)

`Linker::link_target` (`src/linker.rs`): a package with zero dependencies of
its own can never hit the realpath problem that requires the hardlinked
`.nermo/` virtual store (see `docs/architecture.md`), so it now gets a
direct symlink straight into the global store instead. On the real
480-package project, exactly half the graph (244/480, checked against its
`.nermo-lock`) qualifies.

| | before | after |
|---|---|---|
| link phase, 480 packages, cold | ~6-8s | **2.26s** |

No change to disk usage (hardlinks already cost zero extra bytes either
way) or to resolve/store phase time — this only reduces the number of
per-file hardlink operations during linking. Verified correctness with real
Node, not just that the links exist: `require('left-pad')` (shortcut
straight to the store) and `require('debug')` (virtual store, whose own
`require('ms')` needs its own `node_modules`) both resolve correctly from
the same install.

## Update — eliminated a redundant metadata fetch per downloaded package

`Store::ensure` was calling `client.version_metadata(name, version)` — a
separate registry request — for every package it downloaded, even though
the resolver had *already* fetched that exact package's `tarball`/
`integrity`/`shasum` moments earlier and it was sitting unused in
`ResolvedPackage`. `ensure`/`ensure_all` now take the already-known `Dist`
directly (as a lazy closure, so `nermo fetch`'s ad-hoc lookups — which have
no resolved graph to draw from — still fetch metadata, but only on an
actual cache miss).

Real, repeated (3 runs each, fresh store/cache every time) cold-install
comparison against `bun`, same manifest (`yargs@^17.7.2`, 16 packages):

| | run 1 | run 2 | run 3 | avg |
|---|---|---|---|---|
| nermo, before this fix | — | — | — | 2.69s (single sample) |
| **nermo, after this fix** | 1.17s | 1.17s | 1.13s | **1.16s** |
| bun | 2.29s | 1.34s | 2.14s | 1.92s |

Store phase alone: 1.42s → 149ms. On a small-to-medium dependency tree
(round-trip count, not raw bytes, dominates), nermo now beats bun on cold
installs in this comparison. This does **not** generalize to every project —
see the next entry.

## Update — pipelining resolve and download: tried, measured, reverted

Hypothesis: since a package's tarball URL is known the moment it resolves
(not after the whole graph finishes), overlapping download with ongoing
resolution should let a large, download-bandwidth-heavy project (the real
480-package project referenced throughout this doc) approach
`max(resolve, store)` instead of `resolve + store`.

Implemented it (a channel from the resolver to a pool of download workers,
started the instant each package resolved). First version had a real bug —
`while let Ok(x) = mutex.lock().unwrap().recv()` keeps the lock guard alive
for the entire loop body, not just the check, which serialized all
"concurrent" download workers into one and made the cold 480-package install
**worse**: 195s vs a 98.8s baseline. Fixed the lock-scoping bug, re-measured:
**102.08s — statistically a wash against the 98.82s baseline**, not the
predicted improvement.

Reverted rather than keep it. The likely reason it didn't help: this
project's dominant cost is raw transfer bandwidth for a handful of huge
native-binary tarballs (esbuild, sharp, workerd, lightningcss variants), and
those aren't discovered early enough in resolution — behind expensive
`optionalDependencies` platform exploration — to get meaningful overlap
before resolution itself finishes. Recorded here because a negative result
from an honest test is worth as much as a positive one: it rules out a
plausible-sounding lever so it doesn't get re-attempted without new
information, and it's evidence the redundant-fetch fix above was verified
the same way — by measuring, not assuming.

## Update — three more hypotheses tested; one kept, two ruled out

Chasing further improvement on the real 480-package project (store phase:
896MB downloaded in ~52s). Tested three ideas, in order:

**1. Raise download concurrency further.** Already tested and ruled out in
an earlier phase (16 vs 48: no measurable difference) — re-confirmed here,
not re-litigated.

**2. Parallel range-request chunking for large files.** Hypothesis: a
single-connection download of a 36MB tarball only achieved 4.6 MB/s, well
below typical broadband, suggesting a per-connection cap that splitting
across multiple connections could beat. Tested directly: 4 parallel
range-chunked connections for the same file took **35s vs 7.3s for a single
connection** — dramatically worse — and the reassembled file didn't even
match the original, because npm's registry redirects to a CDN and a naive
`curl -r` range request doesn't survive that redirect correctly. Ruled out;
not worth building.

Separately, the math argues against a per-connection cap being the real
constraint anyway: 896MB in 52s at 16-way concurrency is ~17.2 MB/s
aggregate — only ~3.7x the single-connection rate, not ~16x. That's the
signature of an aggregate bandwidth ceiling (this network's real capacity to
`registry.npmjs.org` at the time of testing), not a per-connection limit —
no amount of added parallelism inside nermo can beat a ceiling that isn't
there because of nermo.

**3. Skip the full metadata document for optionalDependencies platform
checks.** `Resolver::resolve_optional` was fetching the *full* multi-version
package document (`Resolver::metadata`) just to check one exact pinned
version's `os`/`cpu` fields — for a project with ~20 platform-sibling
packages per native tool (esbuild, sharp, lightningcss, workerd), of which
only one is ever a match, that's the full document for every one of the ~19
rejected candidates. Confirmed the size difference before writing code:
`@esbuild/darwin-arm64`'s full document is 106,747 bytes; the single version
actually needed is 1,795 bytes — 59x smaller. Since platform-variant
optional dependencies are always pinned to an exact version, switched that
path to the small single-version endpoint (falling back to the full
document only for the rare non-exact range).

Kept — real, reproducible improvement, confirmed with two separate runs:

| | resolve phase |
|---|---|
| before | 43.2s |
| **after** | **38.75s, 38.86s** (two runs) |

A genuine ~10% cut, smaller than the 59x payload reduction might suggest —
consistent with round-trip *latency*, not payload *size*, being the
remaining dominant cost per request (the same conclusion the abbreviated-
metadata-format fix pointed to earlier in this document). Total install time
on this project is still dominated by the store phase's bandwidth ceiling
(§2 above), which is a property of the network, not something further
software changes here can fix.

## Update — the actual answer to "why is bun still faster": we never asked for gzip

Prompted by a fair, pointed question ("if the network is really saturated,
why does bun move the same bytes faster on the same network?"). If a
network ceiling were the whole story, both tools transferring the same data
over the same pipe should take similar time — they didn't (56-63s bun vs
92-96s nermo), so something else had to be different. Two experiments
resolved it:

**Confirmed the earlier "network ceiling" conclusion was still correct, but
incomplete.** Single-connection throughput to a completely unrelated CDN
(nodejs.org via Cloudflare, nothing to do with npm) matched npm's ~4.8 MB/s
exactly, and 8 parallel connections to that same unrelated CDN achieved
**3.91 MB/s — worse than one connection**. That part of the reasoning holds:
raw tarball transfer for large files really is bandwidth-bound on this
network, independent of which CDN or how many connections.

**But the resolve phase isn't a large-file transfer problem — it's many
JSON metadata requests, and `Cargo.toml` never enabled reqwest's `gzip`/
`brotli` features.** That means every single metadata request went out
without `Accept-Encoding`, and the registry had no reason to compress its
response. Confirmed directly: `wrangler`'s metadata document is 16.27MB
uncompressed vs 2.0MB gzip-compressed — an 8x difference, for identical
data. This wasn't a network limitation at all; it was nermo never asking
for the obvious optimization every other npm-ecosystem tool uses by
default.

Fix: `reqwest = { features = ["blocking", "json", "gzip", "brotli"] }` —
no code changes needed, `reqwest` handles the header and transparent
decompression internally.

Result on the real 480-package project, confirmed with two runs:

| | resolve phase | total install |
|---|---|---|
| before | 38.94s | 92.51s |
| **after** | **10.27s, 10.38s** | **~71s** |
| bun (same run) | — | 62.84s |

The bun gap closed from **1.6x to ~1.13x**. Store phase (large tarball
downloads, already gzipped `.tgz` files where HTTP-layer compression barely
applies) is unaffected, as expected — still the genuine bandwidth-bound
part. This is the real lesson from the whole investigation: the "network
ceiling" conclusion wasn't wrong, it was scoped to the wrong phase — it
correctly described the store/download phase, and got incorrectly
generalized to the resolve phase without checking whether resolve was
actually bandwidth-bound at all (it wasn't; it was compression-bound).

## Update — the resolver was resolving more distinct versions than necessary

Asked directly to instrument bytes downloaded (not just wall-clock time),
using real network-interface byte counters (`netstat -ib`) around both
tools on the identical real project: **nermo transferred 317.2MB, bun
transferred 244.3MB** — a 73MB, ~23% gap neither compression nor bandwidth
explains, since both tools hit the same registry over the same connection.

Diffed the actual resolved package sets (nermo's `.nermo-lock` vs bun's
`bun.lock`, tolerant-parsed since it's JSONC) rather than guessing. Two
findings:

- Bun's lockfile has *more* total entries (589 vs 460) but 129 of them are
  platform-variant siblings it *records* (for cross-platform lockfile
  portability — something `docs/lockfile.md` already flags as a gap in
  nermo's own lockfile) without *downloading* most of them.
- **34 packages resolve to meaningfully different versions** — not
  randomly: `semver` (nermo 7.8.5 vs bun 6.3.1), `source-map` (0.7.6 vs
  0.6.1), `picomatch` (4.0.7 vs 2.3.2). The pattern: nermo's resolver always
  independently picks the highest version satisfying each edge's range;
  real npm/pnpm/bun resolvers reuse an already-resolved compatible version
  from elsewhere in the graph when one exists, rather than minting a new
  one. Confirmed in the code: `resolve_one` called `pick_version` against
  the full registry unconditionally, with no check for an existing
  compatible resolution first.

Added `Resolver::find_reusable_version`: before resolving a fresh version
for an edge, check whether an already-resolved version of that package
already satisfies the edge's range (mirroring `pick_version`'s own
exact-vs-semver-range logic so behavior stays consistent), and reuse it
instead of minting a new one. Applied to both regular dependency resolution
and `resolve_optional` (where it has a bonus effect: two different parent
versions both needing the same platform-variant binary now skip the second
platform-check network round trip entirely, not just the download).

Real result on the same project, confirmed with real bytes, not estimates:

| | before | after |
|---|---|---|
| Distinct packages | 480 | 477 |
| Bytes downloaded | 317.2MB | 306.1MB |

A real, correctly-earned ~3.5% reduction — smaller than the full 73MB gap,
and worth being honest about why: several of the 34 version differences
trace back further than a single leaf-level reuse check can fix. nermo
resolving a *newer parent package* (because it always picks max-satisfying
at every level) can mean that parent declares a genuinely newer transitive
requirement (`semver: ^7.0.0`) that an older, bun-resolved version
genuinely doesn't satisfy — that's not a missed reuse check, it's a
cascading version-selection difference propagating through the whole
graph. Fully closing that gap would mean replicating substantially more of
real npm/yarn/pnpm's hoisting and dedup-preference algorithm, which is one
of the most heavily-engineered, complex parts of any real package manager
— a legitimately larger undertaking than this fix, not attempted here.

## Update — found and fixed the real byte-gap cause: optionalDependencies override was ignored

A cold-install run on the real ~480-package project showed nermo pulling
*more* bytes than before the dedup fix should have allowed: 475 packages /
364.5MB vs bun's 462 packages / 266.9MB (a ~1.37x byte gap, worse than the
~1.23x measured earlier). Diffing the actual resolved package sets (not
just counts — bun's own lockfile lists every optionalDependencies platform
variant for reproducibility even though it only *installs* the matching
one, so raw lockfile entry counts aren't comparable) found the real cause:
nermo had downloaded all 22 platform binaries of `esbuild@0.18.20`
(`@esbuild/linux-arm64`, `@esbuild/win32-x64`, ... every OS/arch), not just
`@esbuild/darwin-arm64`.

Checked esbuild@0.18.20's actual npm registry metadata directly
(`registry.npmjs.org/esbuild/0.18.20`): it lists every platform package in
**both** `dependencies` and `optionalDependencies` simultaneously. Per npm
semantics, an `optionalDependencies` entry overrides a same-named
`dependencies` entry — but `resolver.rs`'s `resolve_one` was building edges
from both maps independently with no exclusion, so every platform binary
also got a `DepEdge::Required` edge that bypassed the platform filter in
`resolve_optional` entirely and downloaded unconditionally.

Fix: exclude any name from the `dependencies` edge list if it also appears
in `optionalDependencies` (`resolver.rs`, `resolve_one`).

Real result, same project, real bytes via `netstat -ib`, real wall time,
fresh empty store both times:

| | before fix | after fix | bun |
|---|---|---|---|
| Packages downloaded | 475 | 454 | 462 |
| Bytes downloaded | 364.5MB | 228.5MB | 266.9MB |
| Cold install wall time | 75.5s | 49.2s | 61.6s |

nermo now transfers **fewer bytes than bun** and completes cold installs
**faster than bun** on this project — the entire byte/time gap chased
across this session turned out to be one platform-filtering correctness
bug, not a fundamental network or architecture disadvantage.

Also ran the two other scenarios recommended alongside this fix, same
project, same populated store/cache:

| Scenario | nermo | bun |
|---|---|---|
| Cold (empty store/cache) | 49.2s | 61.6s |
| Warm store, no `node_modules` | 1.96s | 3.54s |
| No-op (already installed, unchanged) | ~14ms | ~35-40ms |

The no-op number also reflects the `Linker::place_link` fix from this same
round (skip remove+recreate when an existing symlink already points at the
correct target) — link time for a true no-op dropped from ~1.95s (a full
node_modules rebuild) to single-digit milliseconds.

Each number above is a single real run, not yet a 5-run median as ideal
benchmarking practice would want — noted here for anyone re-verifying, not
papered over.

## Update — tried parallelizing the link phase; no real win, reverted

Tried running `ensure_virtual_entry`/`place_link` across packages
concurrently via the same `parallel_for_each` helper the resolver already
uses, at concurrency 4/8/16/32/64. Result: noisy 1.7-2.9s range with no
consistent improvement over the sequential ~1.96s baseline, and *worse* at
higher concurrency — the same lesson as the earlier resolver-concurrency
test: this workload's cost is filesystem/journal I/O, not CPU-parallelizable
work. Reverted to sequential rather than keep unhelpful complexity.

## Update — found the real warm-install bottleneck: syscall count, fixed with clonefile()

The link phase for the real project does ~30,000 individual `fs::hard_link`
calls plus a matching number of `walkdir` `lstat`s (454 packages, 29,843
files across the store). That syscall volume — not code structure — was
the actual ~2s cost; this is also why parallelizing it didn't help, since
concurrent threads don't reduce total syscalls and APFS appears to
serialize much of this at the journal level regardless of thread count.

Also found and removed one genuinely redundant syscall along the way:
`hardlink_tree`'s file branch called `fs::create_dir_all(parent)` for every
single file, even though `WalkDir`'s default top-down order guarantees the
parent directory's own entry (which creates it) is always visited first.
Measured effect alone: negligible (~2.0s -> ~1.95s) — not the real cost,
but genuinely dead work, so kept the removal.

The real fix: macOS/APFS has `clonefile()` (what `cp -c` uses under the
hood) — one syscall clones an entire directory tree copy-on-write, at zero
extra disk cost (same guarantee a hardlink already gives), independent of
file count. Added `try_clonefile` (`linker.rs`), a small `unsafe extern
"C"` binding — no new crate; `clonefile()` isn't in std but is one function
in libSystem — gated `#[cfg(target_os = "macos")]` with a `false`-returning
stub elsewhere so `hardlink_tree` remains the deliberate fallback on
Linux/Windows and for any clonefile failure (e.g. crossing a filesystem
boundary). `ensure_virtual_entry` now tries `try_clonefile` first and only
falls back to the walk+hardlink loop if it returns false; the store's own
completion-marker file (which `hardlink_tree` deliberately excludes) is
removed after a successful clone since clonefile copies everything.

Verified this isn't a benchmarking artifact of the kind caught earlier this
session (forking one process per *file* measures fork overhead, not real
cost): measured `clonefile()` two ways before touching any Rust code —
subprocess-per-*package* (233 calls to `cp -c`, matching one call per
virtual-store entry) took 1.81s, barely better than baseline, because
233 forks still costs real time; a raw `clonefile()` syscall in-process via
Python `ctypes` (no fork at all) for the same 233 real packages took
0.19s. The in-process number is what the Rust implementation actually gets,
since it's a direct FFI call with no subprocess involved.

Real result, same project, real timed runs, store pre-populated
(warm-store scenario: populated store, no `node_modules`):

| | before | after | bun |
|---|---|---|---|
| Link phase | ~1.95s | ~270-340ms | — |
| Total warm-store install | ~1.96s | ~0.28s (steady state) | 3.54s |

Verified correctness, not just speed: diffed a cloned virtual entry's
`package.json` byte-for-byte against the store original (identical),
confirmed permissions still read-only (0444, same as the pre-existing
hardlink path — clonefile preserves source attributes, so no regression
there), and confirmed a real `node -e "require(...)"` resolves a
leaf-package symlink nested three levels down
(`@react-router/dev` -> `lodash`) to working, correct content.

Cold-install and no-op numbers are unaffected by this change (cold is
download-bound, no-op already skips `ensure_virtual_entry` entirely via
the marker check) — confirmed no-op still completes in ~25ms.
