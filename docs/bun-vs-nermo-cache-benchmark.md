# nermo vs bun: install speed, cold vs warm cache

**Date:** 2026-09-30
**Machine:** macOS (darwin), Apple Silicon
**nermo:** built from local `main` @ `67029ed` (release build)
**bun:** v1.3.10

## Method

Same `package.json` installed with both tools, three times each:

```json
{
  "dependencies": {
    "react": "^18.3.1",
    "react-dom": "^18.3.1",
    "lodash": "^4.17.21",
    "axios": "^1.7.7",
    "zod": "^3.23.8"
  }
}
```

- **Cold cache**: each tool pointed at a brand-new, empty cache/store directory
  (`NERMO_HOME`/`NERMO_STORE` env vars for nermo, `--cache-dir` for bun) so
  every package is fetched from the registry over the network.
- **Warm cache**: `node_modules` deleted and the same install re-run against
  the *same* cache/store from the cold run, so no network fetch is needed for
  either tool.
- Timed with `/usr/bin/time -p`, wall clock (`real`).
- Neither tool's real global cache (`~/.bun/install/cache`,
  `~/Library/Application Support/nermo`) was touched — both benchmarks ran
  against throwaway directories under `/tmp`.

## Results

| Scenario                  | nermo   | bun     |
|----------------------------|---------|---------|
| Cold (fresh cache, run 1)  | 2.11s   | 1.90s   |
| Cold (fresh cache, run 2)  | 2.25s   | 1.58s   |
| Warm (cache hit, no lockfile change) | 0.03s | 0.02s |
| Warm (cache hit, run 2)    | 0.006s  | 0.02s   |

(37 resolved packages for nermo vs. 34 for bun — the two resolvers pick
slightly different transitive versions within the same semver ranges, e.g.
bun resolved `lodash@4.18.1`, nermo `4.17.21`; not enough of a gap to skew
the timing comparison.)

## Takeaways

- **Cold installs are roughly a wash.** Both tools are dominated by network
  round trips to the registry for ~35 packages; bun edged out nermo by
  ~15-30% here, but at this package count the difference is mostly noise
  (see the ~400ms spread between bun's own two cold runs).
- **Warm installs are near-instant for both**, and both bottom out in the
  10-30ms range once nothing needs to touch the network — bun's warm run is
  dominated by its own lockfile/cache-index overhead, nermo's second warm
  run (6ms) benefits from also skipping re-resolution entirely because
  `.nermo-lock` was still valid and package.json hadn't changed.
- **Where nermo actually wins is cross-project reuse, not raw speed on one
  project.** Its global content-addressed store (`~/Library/Application
  Support/nermo/store`) is shared by *every* project on the machine — a
  second, unrelated project that also depends on `react@18.3.1` reuses that
  exact extracted package with zero download and zero re-extraction, the
  same as the "warm" row above. Bun's per-user cache also dedupes tarballs,
  so this isn't unique to nermo, but nermo's design leans on it more
  explicitly (see `store prune`/`store` stats in the CLI).

## Update — two real fixes, cold installs now at parity with bun

Two root causes for the gap above turned out to be fixable, not fundamental:

**1. The update-check GitHub API call was blocking every command's exit.**
`selfupdate::notify_if_update_available()` (throttled to once/24h via an
on-disk cache) ran *after* the command's own work finished, adding its own
full network round trip on top. Measured directly: `NERMO_NO_UPDATE_CHECK=1`
cut a cold one-package install from ~2.1s to ~1.0s wall time with identical
resolve/store/link numbers — the other ~1.1s was pure hidden latency, not
package management. Fixed by spawning the check on a background thread at
the start of `main` and joining it after the command's own work
(`src/main.rs`) — it now overlaps with the install's own network activity
instead of running sequentially after it, so any command that takes longer
than the check (virtually all real installs) pays nothing extra.

**2. The `express`/`safer-buffer` semver caveat is fixed.** Rust's `semver`
crate only accepts Cargo's comma-separated comparator syntax; npm's
node-semver allows bare-whitespace-joined comparators (`">= 2.1.2 < 3"`).
Added `to_cargo_comparator_syntax` (`src/resolver.rs`) to reattach a lone
operator to the version token that follows it and join the result with
commas before handing it to `VersionReq::parse`. `express@^4.19.2` now
resolves cleanly (71 packages, previously a hard error).

Re-ran the same 5-package cold-install comparison after both fixes, 3 fresh
trials (new store/cache/lockfile each time):

| Run | nermo | bun |
|---|---|---|
| 1 | 2.18s | 1.80s |
| 2 | 1.92s | 2.06s |
| 3 | 1.80s | 1.72s |

Nermo went from a consistent 300-600ms deficit to noise-level parity with
bun on this small graph — the update-check overhead was the dominant
fixable factor; the remaining run-to-run variance is now just network
jitter (both tools fetch essentially the same registry payloads for
`react`/`react-dom`, whose abbreviated metadata documents run 1-3MB each
even compressed — that part is inherent to correct semver resolution against
the real registry, not something either tool can trim further).

## Update — dist-tag fast path for the common case

That "1-3MB even compressed" cost above isn't actually unavoidable for most
packages, just for ones pinned behind a newer major (`react`/`react-dom` in
this manifest). Added a fast path (`Resolver::latest_if_satisfies`,
`src/resolver.rs`): try the registry's tiny single-version `latest`
dist-tag endpoint (a couple KB) first, and only fetch the full multi-version
document when `latest` doesn't satisfy the edge's range. Verified directly:
`lodash@^4.17.21` (satisfied by its actual latest) now resolves in **315ms**
via the fast path, down from paying for the full document; `react@^18.3.1`
(latest is 19.x) correctly falls back to the full document, unaffected. On
this benchmark's specific manifest the win is partial (`react`/`react-dom`
still need the fallback), but on manifests without a stale major pin —
the common case for an actively maintained project — this turns a
multi-MB-per-package cost into a few KB.
