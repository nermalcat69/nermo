# Scoping a JS runtime — is "beat Bun's runtime numbers" realistic, and how

nermo today is a package manager (resolve/download/link `node_modules`), the
same job as `bun install`. The numbers being targeted here — Express req/s,
Postgres queries/s, WebSocket msgs/s — measure something entirely different:
**Bun as a JavaScript runtime** (its own JS engine, event loop, HTTP/TLS
stack, WebSocket server, Postgres driver). This doc scopes what building
that would actually take, honestly, before any code gets written.

## What those specific numbers are actually measuring

| Benchmark | What's really being timed |
|---|---|
| Express hello-world req/s | HTTP parse + socket read/write syscall overhead + per-request JS object allocation/GC, at 50 concurrent connections |
| Postgres 100-in-flight queries/s | Wire-protocol driver efficiency + connection/query pipelining, not JS at all |
| WebSocket broadcast msgs/s | Per-message framing/syscall overhead across 32 sockets, JS dispatch cost per `send`/`publish` call |

None of these are "JS is fast" benchmarks in the sense of tight numeric
loops — they're **dispatch and I/O overhead per operation**, at the scale of
tens of thousands of operations/sec. That means the engine's JIT matters
less here than: syscall batching (io_uring vs epoll vs kqueue), how many
allocations happen per request, and how much the HTTP/WS/Postgres code path
has been hand-tuned. This is good news and bad news — good, because it means
a well-engineered I/O layer matters as much as the JS engine choice; bad,
because "well-engineered" here means the kind of profiling-driven,
allocation-counting work Bun's team has spent actual years on.

## The four things a runtime needs that nermo has none of today

1. **A JS/TS engine.** Writing one from scratch is not a realistic option —
   V8 (Node, Deno), JavaScriptCore (Bun, Safari), and to a lesser extent
   SpiderMonkey each represent 15-20+ years of continuous engineering by
   large teams. The real choice is which existing engine to *embed*:
   - **V8** via the `v8` Rust crate (the same one Deno uses). Mature Rust
     bindings, real JIT performance, but a genuinely heavy build (V8 itself
     is a multi-hour C++ build unless using prebuilt static libs — the `v8`
     crate ships these but they're large and platform-specific), and the
     API surface for embedding is large and C++-shaped even through the
     Rust wrapper.
   - **JavaScriptCore** — what Bun itself embeds. No mature, maintained Rust
     binding exists; Bun's own bindings are hand-written Zig↔C++. Doing
     this from Rust means writing and maintaining a C++ FFI layer against
     WebKit's JSC ourselves — a substantial, ongoing maintenance burden
     independent of the runtime logic itself.
   - **QuickJS** via `rquickjs` (safe Rust bindings). Small, pure-C engine,
     easy to embed, no giant build step. The real cost: **interpreter only,
     no JIT.** Fine for I/O-dispatch-bound workloads like the benchmarks
     above (where JS execution per request is trivial), but this will lose
     badly the moment a workload does real JS compute — worth being
     explicit that "beats Bun on hello-world req/s" and "beats Bun on JS
     compute" are different claims with different engine requirements.
   - **Boa** — a JS engine written in pure Rust. Same interpreter-only
     ceiling as QuickJS, with the advantage of zero FFI/C toolchain at all
     (trivial to embed, cross-compile, and audit) and the disadvantage of
     being less mature/spec-complete than QuickJS.

   **Recommendation for an MVP:** start with `rquickjs`. It's the only
   option here compatible with "a small team can actually finish embedding
   it in weeks, not quarters," and the target benchmarks are dispatch-bound
   enough that an interpreter's lack of JIT may not be the dominant cost —
   this should be measured directly (a "hello world, JSON.stringify one
   object" loop under QuickJS vs V8) before committing either way, not
   assumed.

2. **An async I/O / event loop layer.** This is the one area Rust
   genuinely starts ahead: `tokio` is a best-in-class, battle-tested async
   runtime with mature epoll/kqueue support and experimental `io_uring`
   support (`tokio-uring`) — the same kernel primitive Bun leans on for its
   Linux numbers. The real work isn't building this (it exists), it's
   **bridging tokio's `Future`/task model into the JS engine's promise and
   microtask queue** so `await fetch(...)` in JS actually drives a tokio
   task — that bridge is genuinely novel integration work regardless of
   engine choice.

3. **The actual server/client implementations.**
   - **HTTP + TLS**: `hyper` (HTTP/1.1 + HTTP/2) and `rustls` are both
     mature, fast, pure/mostly-Rust. Realistic to get a competent HTTP
     server quickly; matching Bun's specific 48k req/s number requires the
     same kind of allocation-per-request profiling work Bun did, not just
     "use hyper and call it done."
   - **Postgres**: `tokio-postgres` already exists and is mature — this is
     the single most "just wire it up" piece of the whole list, not a
     research problem.
   - **WebSockets**: `tokio-tungstenite` is mature and sufficient for a
     first version; Bun's 4.17M msgs/s "publish" number specifically
     reflects a broadcast fast path (one write fanned out to many sockets
     without per-socket JS callback overhead) that would need its own
     explicit design, not something a generic WS crate gives for free.

4. **A JS-facing API surface.** Even a minimal runtime needs `fetch`,
   `Request`/`Response`, `console`, `process`, timers, and a module
   loader/resolver (plus TS stripping, since "just run TS files" is table
   stakes for both Bun and Deno now). None of this is hard individually;
   collectively it's the same "long tail of small APIs" work that ate years
   of Node/Deno/Bun's own history, especially once real-world code expects
   **Node compatibility** (`require()`, `Buffer`, `fs`, `http`, `events`,
   npm packages that assume Node's exact module semantics) rather than a
   clean-room API. Express itself, named directly in the pasted benchmarks,
   only runs at all because Bun implements enough of Node's `http` module
   surface for it — that compatibility layer is a project in its own right,
   separate from "can execute JavaScript fast."

## Honest sizing

| Phase | Scope | Realistic effort (small team) |
|---|---|---|
| 0 | Embed an engine, run `console.log("hi")`, a `setTimeout` | 1-2 weeks |
| 1 | Minimal event loop bridge (tokio ↔ engine promises), `fetch`, a bare TCP/HTTP server, no Node compat | 1-2 months |
| 2 | Enough Node `http`/`Buffer`/`events`/module-resolution compat to run unmodified Express apps | 3-6+ months, open-ended (long tail of edge cases) |
| 3 | Postgres client bindings exposed to JS, WebSocket server with a broadcast fast path | 1-2 months on top of Phase 1 (these two are largely independent of Node compat) |
| 4 | Profiling-driven tuning to actually *match or beat* Bun's specific per-request allocation/syscall numbers | Open-ended — this is the part Bun has spent years on and is not a fixed-scope task |

Phases 1 and 3 (a native, non-Node-compat API — closer to `Bun.serve()` or
Deno's own APIs than to Express) are the realistic near-term target and
would let early progress be benchmarked honestly against Node/Deno on
*nermo's own* API surface. Phase 2 (Express compatibility specifically, as
named in the pasted benchmark) is the long pole and should be treated as a
separate, later decision — not bundled into an initial scope — since it's
where Bun's own multi-year investment shows up most.

## Recommendation

1. Don't start with Express compatibility — start with a native HTTP API
   (own the shape of the API, sidestep Node's `http` module semantics
   entirely, same choice Bun itself made with `Bun.serve()`).
2. Prototype Phase 0 with `rquickjs` specifically to get a real, measured
   answer to "does the lack of a JIT actually matter for I/O-dispatch-bound
   workloads like these benchmarks" before committing to the much heavier
   V8-embedding path.
3. Treat this as a genuinely separate project from nermo-the-package-manager
   — different crate, different release cadence, no shared code beyond
   maybe `reqwest`/`tokio` as dependencies — so ongoing package-manager work
   (today's cold-install/nermox work) isn't blocked on or destabilized by
   runtime work.
4. Re-benchmark against Node at the end of each phase, not against Bun's
   specific numbers — Bun's numbers are the Phase-4 target, not a Phase-1
   sanity check, and treating them as an early gate will make every honest
   milestone look like a failure.
