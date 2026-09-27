# NERMO: Research on Bun's Installation Architecture and Performance

**Technical Research Document | Package Manager Engineering | September 2026**

This research document examines why `bun install` is fast, how its internal architecture works, and how you can incorporate similar optimizations into **Nermo**, your Rust-based npm package manager.

It focuses particularly on your goal of creating a shared, global package store that allows developers to reuse dependencies across projects without repeatedly downloading or storing identical packages.

The research covers Bun's package resolution, caching, filesystem operations, concurrency, lockfiles, dependency linking, networking, and installation pipeline. It also proposes a technical architecture for implementing these ideas in Rust.

---

# 1. Executive Summary

Bun is designed to make JavaScript package installation significantly faster than traditional package managers such as npm and Yarn.

Its performance comes from several complementary engineering decisions:

1. **Native implementation:** Bun is written primarily in Zig, allowing it to control memory allocation, filesystem operations, concurrency, and networking without relying on a JavaScript runtime for its package manager.
2. **Global package cache:** Downloaded packages are stored centrally and reused across projects.
3. **Optimized filesystem operations:** Bun uses hardlinks on Linux and Windows and copy-on-write clones on macOS.
4. **Efficient dependency resolution:** Lockfiles allow Bun to avoid repeatedly resolving dependency trees.
5. **Parallel downloads:** Independent packages can be fetched concurrently.
6. **Optimized tarball extraction:** Package extraction is designed to minimize unnecessary filesystem operations.
7. **Efficient metadata caching:** Registry responses are cached in a binary format to reduce parsing and storage overhead.
8. **Incremental installation:** Bun checks existing dependencies and avoids reinstalling packages that are already present.
9. **Reduced filesystem overhead:** Bun minimizes unnecessary file copying and metadata operations.
10. **Installation strategies:** Bun supports both traditional hoisted installations and isolated dependency layouts.

Bun's official documentation advertises installation speeds of up to 25 times faster than npm in certain scenarios. However, this is a maximum advertised comparison, not a universal result. Performance varies according to the project, operating system, network, cache state, and installation strategy. ([Bun][1])

For Nermo, the most important insight is that **downloading packages is only one part of installation performance**.

A package manager can download a dependency quickly and still spend considerable time extracting thousands of files, creating directories, resolving dependencies, and populating `node_modules`.

Nermo should optimize the entire installation pipeline.

## 1.1 Recommended architecture for Nermo

The proposed architecture consists of the following components:

| Component             | Responsibility                             | Priority |
| --------------------- | ------------------------------------------ | -------- |
| Rust CLI              | Command-line interface and orchestration   | Critical |
| Dependency resolver   | Resolve npm dependency graphs              | Critical |
| Lockfile engine       | Store and reproduce exact dependency trees | Critical |
| Global package store  | Store reusable package contents            | Critical |
| Package downloader    | Download registry tarballs concurrently    | Critical |
| Tarball extractor     | Extract packages efficiently               | Critical |
| Filesystem linker     | Link packages into projects                | Critical |
| Installation planner  | Determine which packages need installation | Critical |
| Metadata cache        | Cache registry metadata                    | High     |
| Integrity verifier    | Verify package contents                    | Critical |
| Workspace manager     | Manage monorepos and workspaces            | High     |
| Garbage collector     | Remove unused cached packages              | High     |
| Background prefetcher | Download anticipated dependencies          | Medium   |
| Benchmarking engine   | Measure installation performance           | High     |

The initial implementation should prioritize the global package store, incremental installation, efficient linking, and a reliable lockfile.

These features directly address your original objective: reducing repeated dependency storage across multiple projects.

---

# 2. Understanding the Performance Problem

Before examining Bun's architecture, it is important to understand what happens during a conventional npm installation.

Consider a project with the following dependencies:

```json
{
  "name": "example-project",
  "version": "1.0.0",
  "dependencies": {
    "react": "^19.0.0",
    "next": "^15.0.0",
    "typescript": "^5.0.0"
  },
  "devDependencies": {
    "eslint": "^9.0.0"
  }
}
```

Installing these four direct dependencies may require hundreds of additional transitive dependencies.

For example, Next.js depends on numerous packages that may themselves have dependencies.

The package manager must determine the complete dependency graph before it can produce a working installation.

## 2.1 The traditional installation pipeline

A simplified npm installation can be represented as follows:

```text
                 package.json
                      |
                      v
             Read dependencies
                      |
                      v
             Contact npm registry
                      |
                      v
           Retrieve package metadata
                      |
                      v
             Resolve dependencies
                      |
                      v
             Build dependency tree
                      |
                      v
             Download tarballs
                      |
                      v
             Verify package hashes
                      |
                      v
             Extract package files
                      |
                      v
             Create node_modules
                      |
                      v
             Link dependencies
                      |
                      v
             Execute lifecycle scripts
                      |
                      v
             Installation complete
```

Every stage has a different performance profile.

For example:

* Registry requests are often network-bound.
* Dependency resolution can be CPU- and memory-intensive.
* Tarball extraction can be CPU- or disk-bound.
* Creating `node_modules` can be dominated by filesystem operations.
* Lifecycle scripts can introduce arbitrary additional work.

The important observation is that improving only one stage does not necessarily improve the total installation time.

## 2.2 Installation time as a performance model

A simplified installation time model is:

$$
T_{install} =
T_{resolve} +
T_{download} +
T_{extract} +
T_{link} +
T_{scripts}
$$

This is a useful conceptual model, but actual installation is more complicated because many operations overlap.

For example, a package manager can download one package while extracting another.

A more realistic model is:

$$
T_{install} \approx
T_{startup} +
T_{critical\ path} +
T_{finalization}
$$

The critical path consists of operations that cannot be completed until their prerequisites are available.

Nermo should therefore minimize both the total amount of work and the time spent waiting for dependencies between stages.

### Implication for Nermo

You should not optimize Nermo exclusively for the number of packages downloaded per second.

Instead, measure:

* Time to resolve the dependency graph.
* Time to download packages.
* Time to extract packages.
* Time to link dependencies.
* Time to verify package integrity.
* Total filesystem operations.
* Peak memory usage.
* Total disk space consumed.
* Time to complete a warm installation.

These measurements will help identify which part of Nermo is actually limiting performance.

---

# 3. Why Bun Is Fast: Native Systems Programming

One of Bun's most important architectural decisions is implementing its package manager in a native language.

Bun is written primarily in Zig, rather than implementing its package manager as a conventional JavaScript application.

This gives it direct control over low-level operations.

## 3.1 The overhead of a JavaScript package manager

A traditional JavaScript package manager runs inside Node.js.

Its execution involves:

* Starting the Node.js runtime.
* Loading JavaScript modules.
* Initializing the package manager.
* Allocating JavaScript objects.
* Parsing JSON.
* Scheduling asynchronous operations.
* Interacting with the filesystem through runtime APIs.

Modern Node.js is highly optimized, and this does not mean JavaScript package managers are inherently slow.

However, package installation is a workload involving many small operations. The overhead of processing each operation can become significant when multiplied across thousands of packages and files.

For example, imagine that a package manager must process 100,000 files.

Even a small amount of additional work per file can add up.

Native code can reduce some of this overhead by using efficient data structures, avoiding unnecessary allocations, and directly invoking operating-system APIs.

## 3.2 Why Rust is suitable for Nermo

Rust provides many of the same architectural advantages for a package manager.

It offers:

* Native machine code.
* Efficient memory management.
* Zero-cost abstractions when used appropriately.
* Multithreading through its standard library and ecosystem.
* Direct access to operating-system functionality.
* Strong compile-time guarantees around memory safety and concurrency.

Rust is particularly suitable for Nermo because the package manager requires precise control over filesystem operations and concurrent tasks.

A possible Nermo architecture could use:

| Task                         | Rust implementation                        |
| ---------------------------- | ------------------------------------------ |
| CLI                          | `clap`                                     |
| Asynchronous runtime         | `tokio`                                    |
| HTTP downloads               | `reqwest`                                  |
| JSON parsing                 | `serde_json`                               |
| Lockfile serialization       | `serde`                                    |
| Hashing                      | `sha2`                                     |
| Archive extraction           | `tar` and `flate2`                         |
| Filesystem traversal         | `walkdir` or custom traversal              |
| Concurrent task coordination | Tokio tasks and semaphores                 |
| Memory-efficient collections | Standard collections or specialized crates |

These are candidate libraries rather than mandatory dependencies. Benchmarking should determine which implementations are appropriate.

## 3.3 Memory allocation

Package managers frequently create temporary objects while processing metadata.

For example, resolving a dependency graph involves storing:

* Package names.
* Package versions.
* Version requirements.
* Dependency relationships.
* Registry metadata.
* Package integrity hashes.
* Resolution decisions.

A poorly designed resolver can create many temporary allocations.

Nermo should minimize unnecessary allocation by using compact representations.

For example, instead of repeatedly allocating package names throughout the resolver, it can intern strings.

A simplified representation might be:

```rust
struct PackageId {
    name_id: u32,
    version_id: u32,
}
```

The strings are stored in a separate table.

Dependency relationships can then reference compact identifiers instead of repeatedly copying strings.

For larger projects, this can reduce memory usage and improve cache locality.

However, this optimization should be introduced only after profiling demonstrates that string allocation or hashing is a bottleneck.

---

# 4. Bun's Global Package Cache

The global cache is one of the most relevant features for Nermo.

Bun stores downloaded packages in a shared cache, normally located at:

```text
~/.bun/install/cache/
```

The location can be changed using the `BUN_INSTALL_CACHE_DIR` environment variable.

A cached package can be reused by subsequent projects when its version satisfies their dependency requirements. ([Bun][2])

## 4.1 The problem with project-local dependencies

Consider a developer working on five projects:

```text
~/projects/
├── website/
├── dashboard/
├── api/
├── admin/
└── mobile/
```

Suppose all five projects depend on React, TypeScript, and several other common packages.

With a conventional project-local installation, each project may contain its own physical copy of those packages.

A simplified directory structure looks like this:

```text
website/
└── node_modules/
    ├── react/
    └── typescript/

dashboard/
└── node_modules/
    ├── react/
    └── typescript/

api/
└── node_modules/
    ├── react/
    └── typescript/

admin/
└── node_modules/
    ├── react/
    └── typescript/

mobile/
└── node_modules/
    ├── react/
    └── typescript/
```

This creates redundant storage.

If each project uses the same version of React, all five projects could potentially reuse identical package contents.

The problem becomes even more significant when projects depend on large frameworks and build tools.

## 4.2 Bun's shared cache

Bun avoids downloading the same package repeatedly by maintaining a global package cache.

Conceptually:

```text
~/.bun/install/cache/
├── react@19.0.0/
├── typescript@5.7.0/
├── next@15.0.0/
└── ...
```

When a project needs a package that is already cached, Bun can reuse it instead of downloading it again.

This eliminates unnecessary network traffic and reduces installation work.

However, a global download cache and a global installation store are not the same thing.

A cache can hold a downloaded package while a project still needs a separate copy in its own `node_modules`.

This distinction is central to understanding Bun's newer global virtual store.

## 4.3 Nermo's global store

For Nermo, I recommend separating the package download cache from the package installation store.

A proposed directory layout is:

```text
~/.nermo/
├── cache/
│   ├── metadata/
│   ├── tarballs/
│   └── temporary/
│
├── store/
│   ├── sha256/
│   │   ├── ab/
│   │   │   └── abcdef123456.../
│   │   └── cd/
│   │       └── cdef123456.../
│   │
│   └── packages/
│
├── indexes/
│   └── package-index.db
│
└── config.toml
```

Each component serves a different purpose.

| Directory         | Purpose                                     |
| ----------------- | ------------------------------------------- |
| `cache/metadata`  | Cached registry responses                   |
| `cache/tarballs`  | Downloaded npm tarballs                     |
| `cache/temporary` | Incomplete downloads and extraction staging |
| `store/sha256`    | Verified, immutable package contents        |
| `store/packages`  | Optional package-name and version mappings  |
| `indexes`         | Fast lookup of cached package contents      |
| `config.toml`     | Global Nermo configuration                  |

The most important design decision is that the store should be content-addressable.

## 4.4 Content-addressable storage

Instead of storing packages only by name and version, Nermo can identify their contents by a cryptographic hash.

For example:

```text
react@19.0.0
    |
    v
sha512 integrity verification
    |
    v
package content hash
    |
    v
store/sha256/ab/abcdef123456.../
```

The store's primary identity becomes the package content rather than the package's name.

This has several advantages.

### Deduplication

If two package references resolve to identical content, Nermo can store that content only once.

### Integrity

The hash can be used to verify that the stored package has not changed.

### Reproducibility

The same package content can be referenced from multiple projects.

### Efficient lookup

A hash-based directory structure can avoid huge directories containing thousands of packages.

### Cache portability

Content-addressable storage makes it easier to transfer or share cached package contents between machines.

For compatibility with npm, Nermo should preserve the registry's integrity information, usually a SHA-512 digest, and verify it before trusting the downloaded package.

A separate internal SHA-256 content key is possible, but it adds hashing work. Initially, Nermo could simply use the verified npm integrity digest as the store identity.

## 4.5 The difference between package identity and package contents

Nermo should distinguish between:

```text
Package identity:
react@19.0.0
```

and:

```text
Package contents:
sha512-<integrity-digest>
```

A package identity maps to an immutable content object.

For example:

```text
react@19.0.0
       |
       v
sha512-abc123...
       |
       v
Immutable package contents
```

This separation is valuable because multiple package identities can theoretically refer to identical content.

It also prevents the store from depending exclusively on package names and version strings.

---

# 5. Bun's Filesystem Optimization

Filesystem operations are among the most important factors in package installation performance.

A JavaScript project may contain hundreds or thousands of packages, each with multiple files and directories.

Extracting and copying all these files can take a substantial amount of time.

Bun uses operating-system-specific mechanisms to avoid unnecessary copying.

Its standard installation backend uses hardlinks on Linux and Windows and copy-on-write cloning on macOS. It can fall back to ordinary file copying when these mechanisms are unavailable. ([Bun][2])

## 5.1 Why copying files is expensive

Imagine that a package contains:

```text
package/
├── package.json
├── index.js
├── index.d.ts
├── README.md
├── LICENSE
└── dist/
    ├── index.js
    ├── index.mjs
    └── index.d.ts
```

A conventional installation may copy every file into the project's `node_modules`.

For a large dependency tree, the process can involve:

1. Creating directories.
2. Opening source files.
3. Reading file contents.
4. Creating destination files.
5. Writing file contents.
6. Closing files.
7. Setting permissions.
8. Creating symlinks where necessary.

When multiplied across tens of thousands of files, this can become expensive.

Even when the disk is fast, the operating system must process many filesystem operations.

## 5.2 Hardlinks

A hardlink is another directory entry pointing to the same underlying filesystem object.

Suppose Nermo has stored a package at:

```text
~/.nermo/store/sha256/abc123/
```

A project could expose its files through hardlinks:

```text
project/
└── node_modules/
    └── react/
        ├── package.json
        ├── index.js
        └── ...
```

The files in `node_modules/react` and the global store would refer to the same underlying file data.

This can reduce copying and physical storage consumption.

### Benefits

* Avoids copying file contents.
* Reduces disk usage.
* Makes package materialization faster.
* Allows multiple projects to reuse the same package files.

### Limitations

Hardlinks generally require the source and destination to reside on the same filesystem.

They also refer to the same underlying file.

If a developer modifies a hardlinked file, the modification is visible through all hardlinks to that file.

This is an important consideration for a package manager.

A shared store must be treated as immutable.

If packages are modified after installation, hardlinks can cause changes to leak between projects.

For that reason, hardlinking should be enabled only when Nermo can guarantee the integrity of its store and explain the consequences of modifying installed dependencies.

## 5.3 Copy-on-write cloning

On macOS, Bun can use filesystem cloning through APFS.

A clone creates a logically independent copy of a file while initially sharing underlying storage blocks.

When one copy is modified, the filesystem creates separate blocks as necessary.

For Nermo, this would provide an attractive compromise:

* Fast package materialization.
* Low initial storage overhead.
* Independent project modifications.
* No requirement to expose mutable shared package files.

However, copy-on-write clones are filesystem-dependent.

Nermo should detect whether the target filesystem supports the operation and fall back safely.

## 5.4 Symbolic links

Symbolic links provide another installation strategy.

Instead of copying package contents, Nermo can create links from the project's dependency directory to a shared package store.

For example:

```text
project/
└── node_modules/
    └── react -> ~/.nermo/store/packages/react/19.0.0
```

The project sees a `react` directory, but its contents reside in the global store.

This can make warm installations exceptionally fast because the package manager needs to create only a small number of links.

Bun's global virtual store uses this approach for isolated installs. Its documentation reports approximately sevenfold faster warm installations in a particular benchmark involving a 1,400-package fixture on macOS. This is a workload-specific result, not a guaranteed speedup for every project. ([Bun][3])

### Benefits

* Very little file copying.
* Small project-local `node_modules`.
* Fast warm installations.
* Shared package contents across projects.

### Limitations

* Symlinks can behave differently across operating systems.
* Some development tools do not handle symlinked directories correctly.
* Node.js module resolution can interact with symlinks in unexpected ways.
* Package modifications affect the shared target unless isolation is provided.
* Windows symlink creation can require additional permissions or developer-mode support.

For Nermo, symbolic linking should be a configurable installation backend rather than the only installation method.

## 5.5 Recommended filesystem strategy

I recommend the following priority order:

| Operating system | Preferred strategy                 | Alternative          |
| ---------------- | ---------------------------------- | -------------------- |
| macOS            | Symlinks to immutable global store | Copy-on-write clones |
| Linux            | Symlinks to immutable global store | Hardlinks            |
| Windows          | Symlinks where supported           | Hardlinks or copies  |

This recommendation assumes Nermo uses a global store and an isolated dependency layout.

For compatibility with traditional `node_modules`, Nermo should also support a hoisted installation mode.

The implementation should detect unsupported operations and use a reliable fallback rather than failing the entire installation.

---

# 6. Bun's Global Virtual Store

Bun's global virtual store is especially relevant to Nermo's design.

It addresses a specific weakness of ordinary cached installations: even when packages are already downloaded, reconstructing `node_modules` can still require considerable filesystem work.

The global virtual store avoids repeating that work.

## 6.1 How the global virtual store works

Consider two projects:

```text
~/projects/
├── project-a/
└── project-b/
```

Both projects depend on the same version of React.

Without a global virtual store:

```text
project-a/
└── node_modules/
    └── react/
        ├── package.json
        ├── index.js
        └── ...

project-b/
└── node_modules/
    └── react/
        ├── package.json
        ├── index.js
        └── ...
```

Even if the package is cached, the package manager may need to materialize its files in both projects.

With a global virtual store:

```text
~/.bun/
└── install/
    └── cache/
        └── links/
            └── react@19.0.0/
                ├── package.json
                ├── index.js
                └── ...

project-a/
└── node_modules/
    └── .bun/
        └── react@19.0.0 -> ~/.bun/install/cache/links/react@19.0.0

project-b/
└── node_modules/
    └── .bun/
        └── react@19.0.0 -> ~/.bun/install/cache/links/react@19.0.0
```

Each project points to the same stored package.

The global store holds the package once, while each project creates only a small number of links.

Bun's global virtual store is currently an opt-in feature, and it applies to the isolated linker rather than the traditional hoisted linker. ([Bun][3])

## 6.2 Why this is particularly useful for Nermo

Your original idea is to allow developers to install dependencies once and reuse them across projects.

The global virtual store directly implements that concept.

For example, imagine a developer working on:

* A Next.js application.
* A React component library.
* A documentation website.
* A VS Code extension.
* A desktop application.

All five projects might use TypeScript and React.

Nermo could store the shared packages once and link them into all five projects.

A project would retain a conventional `node_modules` directory, but the actual package contents would live in a shared location.

This allows developers to keep using familiar JavaScript tooling while reducing repeated storage.

## 6.3 The installation algorithm

A simplified Nermo installation algorithm could look like this:

```text
START
  |
  v
Read package.json
  |
  v
Read nermo.lock
  |
  v
Validate dependency graph
  |
  v
Check global package store
  |
  +-------------------------+
  |                         |
  v                         v
Package exists          Package missing
  |                         |
  v                         v
Verify package          Download tarball
  |                         |
  |                         v
  |                    Verify integrity
  |                         |
  |                         v
  |                    Extract package
  |                         |
  |                         v
  |                    Add to global store
  |                         |
  +------------+------------+
               |
               v
       Build installation plan
               |
               v
       Create project links
               |
               v
       Complete installation
```

A production implementation needs additional safeguards, including atomic installation, concurrent cache access protection, and cleanup of incomplete operations.

Nevertheless, this illustrates the core design.

## 6.4 The warm-install advantage

A warm installation occurs when the packages required by a project are already available locally.

For example:

```bash
nermo install
```

If all required package contents already exist in the global store and the lockfile is unchanged, Nermo should not download or extract them again.

It should simply verify the relevant installation state and create any missing project links.

This makes warm installations an important performance target.

A good implementation should also avoid unnecessarily recreating links that are already correct.

---

# 7. Dependency Resolution and Lockfiles

Dependency resolution is another major part of package installation.

The package manager must determine which package versions satisfy all dependency constraints.

For example:

```json
{
  "dependencies": {
    "react": "^19.0.0",
    "next": "^15.0.0"
  }
}
```

The resolver must consider the dependencies of React, Next.js, and every transitive dependency.

## 7.1 Why dependency resolution can be expensive

Resolving packages requires examining version constraints and dependency relationships.

A package may have:

* Dependencies.
* Optional dependencies.
* Peer dependencies.
* Platform-specific dependencies.
* Version ranges.
* Prerelease versions.
* Registry metadata.

The resolver must determine a valid dependency graph while handling possible conflicts.

For example:

```text
project
├── package-a
│   └── shared-package@1.x
└── package-b
    └── shared-package@2.x
```

The package manager may need to install both versions.

It must also ensure that the resulting directory structure supports the expected module resolution behavior.

## 7.2 Lockfiles eliminate repeated resolution

A lockfile records the dependency versions and relationships selected during a previous installation.

For example, a simplified lockfile might contain:

```json
{
  "lockfileVersion": 1,
  "packages": {
    "react": {
      "version": "19.0.0",
      "integrity": "sha512-..."
    },
    "typescript": {
      "version": "5.7.0",
      "integrity": "sha512-..."
    }
  }
}
```

This is only an illustrative structure, not a complete npm-compatible lockfile.

With a valid lockfile, Nermo can avoid resolving every dependency from scratch.

It can instead reconstruct the already-resolved dependency graph.

Bun similarly uses its lockfile to reproduce dependency versions and supports frozen-lockfile installation for reproducible builds. ([Bun][4])

## 7.3 Nermo's lockfile design

I recommend creating a text-based lockfile for Nermo's initial release.

For example:

```toml
lockfile_version = 1
nermo_version = "0.1.0"

[[packages]]
name = "react"
version = "19.0.0"
integrity = "sha512-..."
resolved = "https://registry.npmjs.org/react/-/react-19.0.0.tgz"

[[packages]]
name = "typescript"
version = "5.7.0"
integrity = "sha512-..."
resolved = "https://registry.npmjs.org/typescript/-/typescript-5.7.0.tgz"
```

A production lockfile must also represent dependency relationships, peer dependency resolution, optional dependencies, workspaces, and platform-specific decisions.

I recommend using a compact package identifier and explicit dependency edges.

For example:

```toml
[[packages]]
id = 1
name = "react"
version = "19.0.0"
integrity = "sha512-..."

[[packages]]
id = 2
name = "next"
version = "15.0.0"
integrity = "sha512-..."

[[edges]]
from = 2
to = 1
type = "dependency"
```

The advantage is that the dependency graph is represented explicitly rather than inferred from directory names.

## 7.4 Frozen installation

Nermo should support a command similar to:

```bash
nermo install --frozen-lockfile
```

In frozen mode:

1. Read the lockfile.
2. Verify that the manifest is compatible with it.
3. Install the exact locked package versions.
4. Avoid updating the lockfile.
5. Fail if the manifest requires a different dependency graph.

This mode is essential for CI/CD environments.

It also enables Nermo to make more aggressive installation optimizations because it does not need to resolve new versions.

## 7.5 Lockfile parsing performance

The lockfile should be designed to minimize unnecessary work.

Potential optimizations include:

* Avoiding repeated string allocations.
* Using compact package identifiers.
* Parsing only required fields during installation.
* Caching the parsed lockfile when its contents have not changed.
* Using efficient serialization and deserialization.
* Avoiding unnecessary lockfile rewrites.

However, a binary lockfile is not automatically faster in every situation.

A text-based lockfile is easier to inspect, debug, review, and maintain in version control.

For Nermo, I recommend starting with a well-designed text format and benchmarking it before considering a binary format.

---

# 8. Incremental Installation

A package manager should not reinstall everything whenever a developer runs the install command.

This is particularly important for a development tool because developers frequently run installation commands after changing branches or updating manifests.

Bun checks existing installed packages and can skip packages whose expected names and versions are already present. Its installation behavior also takes advantage of the lockfile and cache state. ([Bun][1])

## 8.1 The problem with full reinstallation

Imagine a project with 1,000 packages.

A developer changes one dependency:

```json
{
  "dependencies": {
    "react": "^19.0.0",
    "next": "^15.0.0",
    "zod": "^4.0.0"
  }
}
```

Only one package requirement has changed.

A poorly designed package manager might unnecessarily reinstall the entire dependency tree.

Nermo should instead determine which packages have actually changed.

## 8.2 Incremental installation algorithm

The proposed algorithm:

```text
1. Read package.json.
2. Read nermo.lock.
3. Compare the manifest with the lockfile.
4. Determine the expected dependency graph.
5. Inspect the existing installation.
6. Identify missing or invalid packages.
7. Check the global store.
8. Download only missing packages.
9. Extract only new package contents.
10. Link only packages that need changes.
11. Remove obsolete links.
12. Finalize the installation.
```

The key optimization is to distinguish between three states.

| Package state             | Action                               |
| ------------------------- | ------------------------------------ |
| Correctly installed       | Do nothing                           |
| Available in global store | Link into project                    |
| Missing from global store | Download, verify, extract, then link |

This avoids repeating expensive operations.

## 8.3 Fast path for unchanged projects

Nermo should have a dedicated fast path for projects whose manifests and lockfiles have not changed.

For example:

```bash
nermo install
```

If the lockfile is unchanged and the installed dependency graph is already correct, Nermo should complete quickly.

It should not need to:

* Contact the registry.
* Download packages.
* Extract tarballs.
* Rebuild the dependency graph.
* Recreate every symlink.

The fast path should perform only the checks needed to establish that the current installation is valid.

A simple manifest hash and lockfile hash can help determine whether a project has changed.

However, hashes alone are not sufficient to prove that the installed filesystem has not been modified.

Nermo should track installation state and verify relevant package entries.

## 8.4 Installation state database

Nermo could maintain a small project-local state file:

```text
node_modules/.nermo-state
```

A possible structure:

```json
{
  "version": 1,
  "manifestHash": "sha256-...",
  "lockfileHash": "sha256-...",
  "storeVersion": 1,
  "installedPackages": {
    "react": {
      "version": "19.0.0",
      "storeId": "sha512-..."
    }
  }
}
```

This state file would allow Nermo to quickly identify whether the previous installation is likely to remain valid.

The file should be treated as an optimization, not as a security boundary.

If the state file is missing or inconsistent, Nermo should reconstruct the installation state rather than blindly trusting it.

---

# 9. Parallel Downloads and Concurrency

Downloading packages sequentially is inefficient when many independent dependencies are required.

Suppose a project needs 100 packages.

If each package takes 100 milliseconds to download and downloads are performed sequentially, the total download time could approach 10 seconds.

If the packages are independent and the network can handle parallel requests, the total time can be significantly reduced.

However, excessive concurrency can overwhelm the network, the registry, or the local filesystem.

## 9.1 Parallel downloads

Nermo should use asynchronous networking and bounded concurrency.

For example:

```text
Download queue
     |
     +--> Package A
     |
     +--> Package B
     |
     +--> Package C
     |
     +--> Package D
     |
     +--> Package E
```

Instead of waiting for each download to finish before starting the next, Nermo can download several packages simultaneously.

Rust's Tokio runtime is a suitable foundation for this design.

A simplified example:

```rust
use tokio::task::JoinSet;
use tokio::sync::Semaphore;
use std::sync::Arc;

async fn download_packages(packages: Vec<Package>) {
    let semaphore = Arc::new(Semaphore::new(16));
    let mut tasks = JoinSet::new();

    for package in packages {
        let semaphore = Arc::clone(&semaphore);

        tasks.spawn(async move {
            let _permit = semaphore.acquire().await.unwrap();

            download_package(package).await
        });
    }

    while let Some(result) = tasks.join_next().await {
        if let Err(error) = result {
            eprintln!("Download failed: {error}");
        }
    }
}
```

This is illustrative pseudocode for the download stage. A production implementation would need explicit error propagation, retries, cancellation, and task scheduling.

## 9.2 Why bounded concurrency matters

Suppose Nermo downloads 500 packages.

Starting 500 simultaneous downloads could lead to:

* Excessive memory consumption.
* Too many open connections.
* Registry rate limiting.
* Increased context switching.
* Disk contention.
* Poor performance on slower machines.

Instead, Nermo should use a configurable concurrency limit.

For example:

```toml
[network]
concurrent_downloads = 16
```

The default should be determined through benchmarks.

Nermo could eventually adapt the concurrency limit according to:

* Available CPU cores.
* Available memory.
* Network performance.
* Registry response times.
* Filesystem throughput.

Initially, a fixed, configurable limit is simpler and easier to debug.

## 9.3 Separate download and extraction concurrency

Downloading and extracting packages have different resource requirements.

Downloads are primarily network-bound.

Extraction is often CPU- and filesystem-bound.

Therefore, Nermo should use separate concurrency limits:

```toml
[install]
concurrent_downloads = 16
concurrent_extractions = 4
concurrent_links = 8
```

These values are illustrative.

A single concurrency limit for every operation would make it difficult to tune the installation pipeline.

## 9.4 Pipeline-based installation

Nermo should allow the download and extraction stages to overlap.

For example:

```text
                 Package queue
                       |
                       v
                 Download stage
                  /    |    \
                 v     v     v
                A      B      C
                 |      |      |
                 v      v      v
              Extract Extract Extract
                 |      |      |
                 v      v      v
                Store  Store  Store
                  \      |      /
                   \     |     /
                    v    v    v
                    Link stage
```

As soon as a package is downloaded and verified, its extraction can begin.

Nermo does not need to wait for every package to download before extracting the first one.

This reduces idle time between stages.

The scheduler should also respect dependency constraints where necessary.

For instance, a package's lifecycle script may depend on other packages being installed.

---

# 10. Tarball Extraction

npm packages are commonly distributed as compressed tarballs.

A package manager must download, verify, decompress, and extract these archives.

This process can involve thousands of filesystem operations.

Bun's installation architecture includes optimizations for tarball extraction, reducing overhead during package installation. Its engineering discussion identifies extraction, system calls, and caching as important parts of the performance problem. ([Bun][5])

## 10.1 Conventional extraction

A simple extraction process might look like:

```text
Download .tgz
     |
     v
Decompress gzip
     |
     v
Read tar entries
     |
     v
Create directories
     |
     v
Write files
     |
     v
Set permissions
     |
     v
Finish extraction
```

A package containing many small files can be expensive to extract because each file requires filesystem operations.

## 10.2 Nermo's extraction strategy

I recommend extracting packages directly into a temporary directory inside the global store's filesystem.

For example:

```text
~/.nermo/store/temporary/
    package-123.tmp/
```

Once extraction and integrity verification are complete, Nermo can atomically move the directory into its final content-addressed location.

For example:

```text
~/.nermo/store/sha512/
    abc123.../
```

This avoids exposing incomplete packages to other installations.

## 10.3 Atomic package publication

The package should not become visible in the global store until it is fully extracted and verified.

The recommended procedure is:

1. Download the tarball to a temporary file.
2. Verify the tarball's integrity.
3. Extract it into a temporary directory.
4. Validate the extracted package structure.
5. Apply the necessary permissions.
6. Flush data when required for durability.
7. Atomically rename the temporary directory into the store.
8. Update the package index.

The final rename should occur on the same filesystem to preserve atomicity.

Nermo should also handle the case where another process has already installed the same package.

In that situation, the second process should verify that the existing store entry is valid and discard its redundant temporary extraction.

## 10.4 Safe tarball extraction

Package extraction is also a security-sensitive operation.

Nermo must protect against:

* Path traversal.
* Absolute paths.
* Unexpected symbolic links.
* Malicious hardlinks.
* Excessive file counts.
* Extremely large extracted files.
* Archive bombs.
* Invalid file permissions.

For example, an archive entry attempting to extract to:

```text
../../../../home/user/.ssh/authorized_keys
```

must never be allowed to escape the temporary extraction directory.

The extractor should normalize and validate every path before writing files.

It should also enforce reasonable resource limits.

Performance optimizations must not weaken extraction security.

---

# 11. Registry Metadata Caching

A package manager needs metadata to resolve dependency versions.

For example, requesting:

```bash
nermo add react
```

may require retrieving information about available React versions.

The npm registry provides package metadata containing version information, dependency declarations, distribution URLs, integrity hashes, and other fields.

Repeatedly downloading and parsing this metadata can introduce unnecessary work.

## 11.1 Bun's binary metadata cache

Bun caches npm registry responses in a binary format.

The cache is stored in files with the `.npm` extension, using hashed package names to avoid creating directories for scoped packages.

Bun documents that this representation loads faster and tends to use less disk space than JSON. ([Bun][6])

This is a useful optimization for Nermo.

## 11.2 Nermo's metadata cache

A proposed cache structure:

```text
~/.nermo/cache/metadata/
├── 12ab34.nmeta
├── 56cd78.nmeta
└── 90ef12.nmeta
```

Each file could store:

* Package name.
* Registry URL.
* Retrieval timestamp.
* Cache expiration.
* Available versions.
* Dependency metadata.
* Distribution URLs.
* Integrity hashes.

The cache should support incremental updates when registry metadata changes.

## 11.3 Binary versus JSON metadata

A binary metadata cache may reduce parsing overhead, but the format should be chosen based on measured performance.

A good initial strategy would be:

1. Retrieve registry JSON.
2. Parse it using `serde_json`.
3. Extract the fields needed for resolution.
4. Store a compact binary representation.
5. Reuse the compact representation on subsequent installs.

This avoids reparsing large registry documents on every installation.

However, Nermo should preserve enough information to handle npm's evolving metadata format.

A rigid binary representation that cannot accommodate new fields could create compatibility problems.

## 11.4 Cache expiration

Registry metadata is not immutable.

New package versions may be published, existing dist-tags may change, and packages can be deprecated.

Nermo should implement cache expiration using registry HTTP caching headers when possible.

It should also support explicit policies:

```toml
[cache]
metadata_ttl_seconds = 300
prefer_offline = false
offline = false
```

A short TTL can improve freshness but increase network requests.

A longer TTL can reduce network traffic but may delay discovery of new releases.

The right default depends on the intended workflow.

For an ordinary install using a lockfile, Nermo often does not need to retrieve current metadata at all.

That is one reason a lockfile is so valuable.

---

# 12. Dependency Linking and Installation Layouts

After packages have been downloaded and extracted, the package manager must make them accessible to Node.js.

This is the purpose of the installation layout.

There are two major approaches:

1. Hoisted installation.
2. Isolated installation.

Bun supports both strategies. Its current documentation describes the isolated strategy as the default for certain new workspace projects and the hoisted strategy for many existing projects. ([Bun][7])

## 12.1 Hoisted installation

A hoisted layout places dependencies into a relatively flat `node_modules` directory.

For example:

```text
node_modules/
├── react/
├── next/
├── typescript/
├── zod/
└── ...
```

This is similar to the traditional npm installation layout.

### Advantages

* Familiar to developers.
* Broad compatibility with existing tools.
* Simple layout for many dependency graphs.
* Suitable for projects expecting traditional npm behavior.

### Disadvantages

* Dependency conflicts can require nested directories.
* Phantom dependencies can become accessible.
* Monorepo dependency isolation is more difficult.
* Deduplication decisions can affect the final layout.

## 12.2 Isolated installation

An isolated installation keeps packages in a structured store and exposes dependencies through symlinks.

For example:

```text
node_modules/
├── .bun/
│   ├── react@19.0.0/
│   ├── next@15.0.0/
│   └── typescript@5.7.0/
│
├── react -> .bun/react@19.0.0/node_modules/react
├── next -> .bun/next@15.0.0/node_modules/next
└── typescript -> .bun/typescript@5.7.0/node_modules/typescript
```

The precise layout depends on the package manager's linker implementation.

The important idea is that packages are stored separately, while symlinks provide the expected dependency paths.

### Advantages

* More predictable dependency resolution.
* Better isolation between packages.
* Reduced accidental access to undeclared dependencies.
* More suitable for large monorepos.
* Compatible with a shared global package store.

### Disadvantages

* Requires more sophisticated linking.
* Some tools assume a traditional flat layout.
* Peer dependencies require careful handling.
* Symlink behavior can introduce compatibility issues.

## 12.3 Recommended strategy for Nermo

Nermo should implement isolated installation as its primary architecture, with a compatibility-oriented hoisted mode.

The global store should be independent of the linker.

This distinction is important.

The same verified package contents should be reusable regardless of whether a project uses isolated or hoisted installation.

For example:

```text
                  Global Store
                       |
          +------------+------------+
          |                         |
          v                         v
   Isolated Linker             Hoisted Linker
          |                         |
          v                         v
   node_modules/.nermo        node_modules/
```

This allows Nermo to improve its storage engine without rewriting its package resolution system.

---

# 13. Peer Dependencies

Peer dependencies are one of the more complicated parts of npm compatibility.

Consider a React component library:

```json
{
  "name": "example-ui",
  "peerDependencies": {
    "react": "^19.0.0"
  }
}
```

The library expects React to be supplied by the consuming project.

A package manager must ensure that the dependency layout allows the library to resolve a compatible React version.

This can become complicated when multiple packages require different peer dependency versions.

## 13.1 Why peer dependencies matter for performance

Peer dependency resolution can increase the complexity of the dependency graph.

A package manager cannot always deduplicate packages solely by name and version.

It may need to consider the peer dependency environment in which a package is installed.

For example:

```text
project
├── package-a
│   └── react@18
└── package-b
    └── react@19
```

Packages that have identical names and versions may need separate installation instances if their peer dependency contexts differ.

## 13.2 Nermo's peer dependency model

I recommend representing peer dependency context separately from package content.

For example:

```text
Package content:
  example-ui@1.0.0

Installation instance:
  example-ui@1.0.0
  peer context:
    react@19.0.0
```

The global content store can still deduplicate the actual package files.

The installation planner can create different dependency instances where required without duplicating the underlying package content.

This separation helps Nermo maintain a compact global store while supporting complex dependency graphs.

---

# 14. Platform-Specific Dependencies

Some npm packages provide different implementations for different operating systems or CPU architectures.

Examples include native packages and optional binary dependencies.

A package manager must consider fields such as:

```json
{
  "os": ["linux", "darwin"],
  "cpu": ["x64", "arm64"]
}
```

It must also account for optional dependencies that may not be installable on every platform.

Bun records normalized platform information in its lockfile and can skip packages that are incompatible with the current target. ([Bun][1])

## 14.1 Nermo's platform selection

Nermo should represent the installation target explicitly:

```rust
struct TargetPlatform {
    os: OperatingSystem,
    architecture: Architecture,
    libc: Option<Libc>,
}
```

The platform selection process should consider:

* Operating system.
* CPU architecture.
* Native binary compatibility.
* Optional dependencies.
* Package-specific installation constraints.

The libc field is particularly useful for Linux because native packages may distinguish between glibc and musl environments.

## 14.2 Cross-platform lockfiles

Nermo should avoid unnecessarily generating different lockfiles for each platform.

Instead, the lockfile should record enough information to make deterministic platform-specific selections during installation.

This allows developers to share a lockfile across Linux, macOS, and Windows.

However, platform-specific package selection must be deterministic and must not silently omit required dependencies.

---

# 15. Lifecycle Scripts and Installation Security

npm packages can define lifecycle scripts such as:

```json
{
  "scripts": {
    "install": "node install.js",
    "postinstall": "node setup.js"
  }
}
```

These scripts may download binaries, compile native code, generate files, or perform other operations.

They can also execute arbitrary code.

Lifecycle scripts can significantly affect installation time.

## 15.1 Why lifecycle scripts are a performance problem

Even if the package manager downloads and extracts every dependency efficiently, a package's installation script can take several seconds or longer.

For example:

* Compiling a native extension.
* Downloading a platform-specific binary.
* Building a package from source.
* Running a code-generation process.

These operations are not necessarily under the package manager's direct control.

## 15.2 Bun's lifecycle script policy

Bun does not automatically execute arbitrary lifecycle scripts from installed dependencies unless those dependencies are trusted.

It supports a `trustedDependencies` field in `package.json` to allow selected packages to run their scripts.

Bun also runs permitted scripts concurrently and supports configuration of the concurrency limit. ([Bun][1])

## 15.3 Nermo's recommended policy

Nermo should implement an explicit trust model.

For example:

```json
{
  "trustedDependencies": [
    "esbuild",
    "sharp"
  ]
}
```

The initial implementation should:

1. Identify packages with lifecycle scripts.
2. Skip untrusted dependency scripts by default.
3. Allow explicit trust through the project manifest or configuration.
4. Execute trusted scripts in a controlled environment.
5. Capture output and exit codes.
6. Support bounded concurrency.
7. Clearly report skipped scripts.

However, compatibility is important.

Some npm packages depend on installation scripts to function correctly.

Nermo should provide clear diagnostics and an explicit way to enable required scripts.

## 15.4 Native dependency optimizations

Bun has special handling for popular native dependencies such as esbuild and sharp.

It can optimize their installation behavior rather than treating every lifecycle script identically. ([Bun][1])

Nermo could eventually implement package-specific installation optimizations.

However, these should be introduced only after the generic lifecycle system is reliable.

A generic mechanism based on trusted packages and explicit installation metadata is a better foundation than hardcoding behavior for many individual packages.

---

# 16. The Importance of System Calls

A major lesson from Bun's engineering work is that package installation can become limited by system call overhead.

This is especially relevant to projects with thousands of dependencies.

Bun's 2025 engineering article describes optimizations involving filesystem system calls, tarball extraction, metadata caching, and parallel execution. ([Bun][5])

## 16.1 What is a system call?

A system call allows a program to request a service from the operating system.

Examples include:

* Opening a file.
* Reading file contents.
* Writing file contents.
* Creating a directory.
* Creating a symbolic link.
* Renaming a file.
* Reading file metadata.

During package installation, these operations can occur thousands of times.

## 16.2 Why minimizing system calls matters

Imagine a package manager extracting 20,000 files.

For each file, it may need to:

1. Validate the archive entry.
2. Create the destination directory.
3. Open the destination file.
4. Write the file.
5. Set its permissions.
6. Close the file.

Even if each operation is individually fast, the aggregate overhead can become significant.

Nermo should therefore reduce unnecessary operations.

Possible optimizations include:

* Avoiding repeated directory existence checks.
* Reusing already-created directories.
* Avoiding redundant permission changes.
* Avoiding unnecessary file metadata reads.
* Batching work where supported.
* Avoiding recreating unchanged links.
* Keeping temporary files on the same filesystem as their final destination.

## 16.3 Directory creation

A naive extractor might repeatedly attempt to create directories for every archive entry.

For example:

```text
package/
package/dist/
package/dist/index.js
package/dist/index.d.ts
package/dist/types/
package/dist/types/index.d.ts
```

The directory `package/dist/` may be encountered multiple times.

Nermo should maintain an efficient set of directories that have already been created.

This avoids repeated filesystem operations.

However, the set should be bounded appropriately for very large packages.

## 16.4 Avoid unnecessary metadata reads

A package manager may repeatedly read `package.json` files to identify package names and versions.

Bun's documentation describes a custom parser that stops once it has found the expected `name` and `version` fields when checking existing installations. ([Bun][1])

Nermo could implement a similar fast path.

For example, when checking whether a package is already installed, it may not need to parse every field in its manifest.

A small, efficient parser that extracts only the required fields could reduce unnecessary work.

Nevertheless, the optimization should not replace full parsing when Nermo needs to validate dependency declarations or other package metadata.

---

# 17. Avoiding Redundant Package Work

A major principle behind a fast package manager is avoiding unnecessary operations.

For Nermo, this should be a core architectural rule.

Consider a project that requires 1,000 packages.

Suppose:

* 900 packages are already in the global store.
* 80 packages are already linked correctly.
* 20 packages are missing from the store.

A naive implementation might reinstall all 1,000 packages.

A more efficient implementation should:

1. Reuse the 900 cached packages.
2. Preserve the 80 correct links.
3. Download only the 20 missing packages.
4. Extract only the 20 new packages.
5. Link the packages that are missing from the project.

This reduces network, extraction, and filesystem work.

## 17.1 A package installation planner

I recommend a dedicated installation planner.

The planner should produce an explicit list of operations:

```rust
enum InstallAction {
    Download {
        package_id: PackageId,
    },
    Verify {
        package_id: PackageId,
    },
    Extract {
        package_id: PackageId,
    },
    Link {
        package_id: PackageId,
        destination: PathBuf,
    },
    Remove {
        destination: PathBuf,
    },
    Skip {
        package_id: PackageId,
    },
}
```

This is a conceptual representation.

The actual implementation should use efficient identifiers and avoid unnecessary path allocations.

The planner can then schedule independent operations concurrently.

## 17.2 Benefits of separating planning and execution

Separating the installation plan from its execution offers several advantages.

### Predictability

Nermo can show what it intends to do before changing the filesystem.

### Testing

The planner can be tested independently from the downloader and linker.

### Dry runs

Nermo can support:

```bash
nermo install --dry-run
```

### Better error recovery

If installation fails, Nermo can identify which actions completed successfully.

### Performance

The planner can skip unnecessary operations before they reach the execution stage.

This architecture also makes it easier to add more advanced features later.

---

# 18. Filesystem Transactions and Crash Recovery

A fast package manager must also be reliable.

Imagine a user loses power while Nermo is installing a package.

If the package manager writes directly into the final store directory, the next installation may encounter a partially extracted package.

This can cause difficult-to-debug problems.

## 18.1 Transactional package installation

Nermo should use a transactional approach.

For each package:

```text
Download
   |
   v
Temporary archive
   |
   v
Integrity verification
   |
   v
Temporary extraction
   |
   v
Package validation
   |
   v
Atomic publication
   |
   v
Global store
```

The package becomes available only after all required operations succeed.

## 18.2 Installation journal

For larger projects, Nermo could maintain an installation journal.

For example:

```text
.nermo/
└── install-journal.json
```

The journal might contain:

```json
{
  "transaction": "install-123",
  "status": "in_progress",
  "completed": [
    "download:react@19.0.0",
    "extract:react@19.0.0"
  ],
  "pending": [
    "link:react@19.0.0"
  ]
}
```

On the next run, Nermo could detect incomplete operations and recover safely.

The journal should not become a performance bottleneck.

For an initial version, it may be sufficient to use atomic temporary directories and rebuild project links from the lockfile.

A journal becomes more useful as Nermo introduces more complex installation transactions.

---

# 19. Package Store Garbage Collection

A global package store introduces a new problem.

Over time, developers may accumulate many packages and versions that are no longer needed.

For example:

```text
~/.nermo/store/
├── react@18.2.0/
├── react@19.0.0/
├── react@19.1.0/
├── next@14.0.0/
├── next@15.0.0/
└── next@16.0.0/
```

Some versions may no longer be referenced by any project.

Nermo should provide a garbage collector.

## 19.1 Reference-based garbage collection

The simplest model is to track which projects reference each package.

For example:

```text
Project A
  ├── react@19.0.0
  └── next@15.0.0

Project B
  ├── react@19.0.0
  └── typescript@5.7.0

Project C
  └── react@18.2.0
```

The global store must retain the packages referenced by any active project.

Packages that are no longer referenced can be considered for removal.

## 19.2 Recommended garbage collection model

Nermo should maintain a store index.

A simplified record might contain:

```json
{
  "storeId": "sha512-abc123",
  "package": "react",
  "version": "19.0.0",
  "lastAccessed": "2026-09-27T10:00:00Z",
  "references": 2
}
```

Reference counts can improve garbage collection performance, but they are vulnerable to becoming inconsistent if a project is deleted or a process crashes.

Therefore, Nermo should periodically reconcile its index with actual project references.

A safer initial design is mark-and-sweep:

1. Discover active Nermo-managed projects.
2. Read their lockfiles.
3. Mark referenced store entries.
4. Identify unreferenced entries.
5. Apply a configurable retention policy.
6. Remove eligible packages.

For example:

```bash
nermo store gc
```

The command should support a dry run:

```bash
nermo store gc --dry-run
```

It should also avoid deleting packages currently being installed.

## 19.3 Retention policies

Nermo could eventually support:

```toml
[store]
max_size = "20GB"
retain_unused_days = 30
```

The garbage collector could prioritize old, unused packages when the store approaches its size limit.

This would help developers manage storage without sacrificing the performance benefits of caching.

---

# 20. Offline Installation

Offline installation is a natural benefit of a global package store.

If all required package contents and metadata are available locally, Nermo should be able to install dependencies without accessing the registry.

Bun supports both offline and prefer-offline installation modes. In offline mode, missing required packages cause an error rather than triggering network access. ([Bun][6])

## 20.1 Nermo's offline mode

Nermo should support:

```bash
nermo install --offline
```

The installation process would:

1. Read the lockfile.
2. Check the global store.
3. Verify that every required package is available.
4. Construct the dependency layout.
5. Complete installation without network access.

If a required package is missing, Nermo should report the exact package.

For example:

```text
error: Offline installation failed

Missing package:
  next@15.0.0

Required by:
  project@1.0.0

Run nermo install with network access to populate the store.
```

## 20.2 Prefer-offline mode

Nermo should also support:

```bash
nermo install --prefer-offline
```

This mode should use cached packages and metadata when possible but retrieve missing data from the registry.

This provides a useful compromise between speed and freshness.

## 20.3 CI caching

The global store could also be cached in CI environments.

For example, a CI workflow could:

1. Restore a cached Nermo store.
2. Run `nermo install --offline --frozen-lockfile`.
3. Build the project.
4. Save newly downloaded packages to the CI cache.

This can reduce network usage and improve repeatability.

However, CI caches must be keyed appropriately, and untrusted caches should not bypass integrity verification.

---

# 21. Nermo's Proposed Installation Architecture

Based on the preceding analysis, I recommend organizing Nermo into independent subsystems.

```text
                         NERMO CLI
                             |
                             v
                    Command Dispatcher
                             |
                             v
                    Project Inspector
                             |
                             v
                    Manifest Parser
                             |
                             v
                     Lockfile Engine
                             |
                             v
                    Dependency Resolver
                             |
                             v
                   Installation Planner
                             |
             +---------------+---------------+
             |               |               |
             v               v               v
       Metadata Cache   Package Store    Project State
             |               |               |
             +---------------+---------------+
                             |
                             v
                    Task Scheduler
                             |
             +---------------+---------------+
             |               |               |
             v               v               v
         Downloader      Extractor         Verifier
             |               |               |
             +---------------+---------------+
                             |
                             v
                       Global Store
                             |
                             v
                       Linker Engine
                             |
                             v
                       node_modules
                             |
                             v
                    Lifecycle Scripts
                             |
                             v
                    Installation Complete
```

The components should communicate through explicit interfaces.

For example, the resolver should not directly manipulate filesystem paths.

It should produce a dependency graph.

The installation planner should convert that graph into installation actions.

The downloader, extractor, verifier, and linker should execute those actions.

This separation will make Nermo easier to test, optimize, and extend.

---

# 22. Recommended Rust Project Structure

A possible project layout:

```text
nermo/
├── Cargo.toml
├── Cargo.lock
├── README.md
│
├── crates/
│   ├── nermo-cli/
│   │   └── src/
│   │       └── main.rs
│   │
│   ├── nermo-core/
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── manifest.rs
│   │       ├── resolver.rs
│   │       ├── lockfile.rs
│   │       ├── planner.rs
│   │       └── platform.rs
│   │
│   ├── nermo-registry/
│   │   └── src/
│   │       ├── client.rs
│   │       ├── metadata.rs
│   │       ├── auth.rs
│   │       └── cache.rs
│   │
│   ├── nermo-store/
│   │   └── src/
│   │       ├── store.rs
│   │       ├── index.rs
│   │       ├── content.rs
│   │       ├── gc.rs
│   │       └── transaction.rs
│   │
│   ├── nermo-extract/
│   │   └── src/
│   │       ├── tarball.rs
│   │       ├── gzip.rs
│   │       ├── paths.rs
│   │       └── security.rs
│   │
│   ├── nermo-linker/
│   │   └── src/
│   │       ├── linker.rs
│   │       ├── isolated.rs
│   │       ├── hoisted.rs
│   │       ├── symlink.rs
│   │       └── hardlink.rs
│   │
│   └── nermo-bench/
│       └── src/
│           ├── main.rs
│           ├── fixtures.rs
│           └── metrics.rs
│
└── tests/
    ├── install/
    ├── resolution/
    ├── workspaces/
    ├── registry/
    └── compatibility/
```

This is a proposed structure rather than a requirement to create every crate immediately.

For the first prototype, you could use a smaller workspace and split crates when the interfaces become stable.

---

# 23. Installation Performance Modes

Nermo should expose a few installation modes that correspond to different developer workflows.

| Mode           | Behavior                                        | Intended use                       |
| -------------- | ----------------------------------------------- | ---------------------------------- |
| Normal         | Resolve and install as needed                   | Everyday development               |
| Frozen         | Install exact lockfile versions                 | CI/CD                              |
| Offline        | Use only locally available data                 | Air-gapped or offline environments |
| Prefer offline | Reuse local cache, download missing packages    | Everyday development               |
| Production     | Omit development dependencies                   | Deployment                         |
| Link-only      | Reconstruct project links from the global store | Fast local recovery                |
| Dry-run        | Show planned operations without installing      | Debugging                          |
| Force          | Reinstall or revalidate packages                | Troubleshooting                    |

Some modes can be combined.

For example:

```bash
nermo install --offline --frozen-lockfile
```

This should perform a deterministic installation without network access, provided all required packages are cached.

A separate command could reconstruct `node_modules` from the global store without resolving versions:

```bash
nermo relink
```

This would be particularly useful if a developer accidentally deletes `node_modules`.

---

# 24. Benchmarking Nermo Against Bun

A package manager should be benchmarked across multiple scenarios.

A single benchmark is not sufficient to establish whether Nermo is faster than Bun.

The two tools may perform differently depending on whether packages are already cached, whether `node_modules` exists, and whether the filesystem supports optimized linking.

## 24.1 Benchmark categories

I recommend measuring at least six scenarios.

| Benchmark              | Description                                     | Main bottleneck                 |
| ---------------------- | ----------------------------------------------- | ------------------------------- |
| Cold install           | Empty cache and no `node_modules`               | Network, extraction, resolution |
| Warm install           | Package cache populated                         | Linking and validation          |
| No-op install          | Everything already installed                    | Startup and validation          |
| Deleted `node_modules` | Global store warm, project dependencies missing | Linking                         |
| One dependency changed | Existing project with a small manifest change   | Incremental planning            |
| Large monorepo         | Many workspaces and shared dependencies         | Resolution and linking          |

These tests will reveal whether Nermo's optimizations work across different workflows.

## 24.2 Example benchmark fixture

Create a project with:

```json
{
  "name": "nermo-benchmark",
  "version": "1.0.0",
  "dependencies": {
    "next": "^15.0.0",
    "react": "^19.0.0",
    "react-dom": "^19.0.0",
    "typescript": "^5.0.0",
    "zod": "^4.0.0",
    "eslint": "^9.0.0"
  }
}
```

Use a committed lockfile and the same dependency versions across all package managers.

For a large-scale benchmark, use a real-world project with hundreds or thousands of transitive dependencies.

## 24.3 Metrics

Nermo should report:

```text
Installation completed

Resolution:       120 ms
Download:         850 ms
Verification:      80 ms
Extraction:       240 ms
Linking:           90 ms
Lifecycle scripts: 0 ms
Total:           1,380 ms
```

The values above are illustrative, not benchmark results.

The important feature is the stage-by-stage breakdown.

Nermo should also record:

* Number of packages resolved.
* Number of packages downloaded.
* Number of cache hits.
* Number of files extracted.
* Number of files linked.
* Number of files copied.
* Total bytes downloaded.
* Total bytes written.
* Peak memory consumption.

This will make performance regressions easier to detect.

## 24.4 Benchmark methodology

For fair comparisons:

1. Use the same machine.
2. Use the same operating system.
3. Use the same project and lockfile.
4. Use the same dependency versions.
5. Separate cold and warm benchmarks.
6. Run each benchmark multiple times.
7. Report median and percentile results.
8. Avoid unrelated background workloads.
9. Record the package manager versions.
10. Record whether lifecycle scripts are enabled.

For warm-install tests, ensure that the caches are actually populated.

For cold-install tests, clear the relevant caches and ensure that the package managers are not reusing previously installed dependencies.

Do not compare one tool's warm installation against another tool's cold installation.

---

# 25. A Practical Development Roadmap for Nermo

I recommend implementing Nermo in phases.

The goal should be to establish a reliable installation engine before introducing advanced optimizations.

## Phase 1: Basic npm compatibility

**Objective:** Build a working package manager.

Implement:

* Read `package.json`.
* Parse dependency declarations.
* Retrieve package metadata.
* Resolve simple semver ranges.
* Download npm tarballs.
* Verify package integrity.
* Extract packages.
* Create a conventional `node_modules`.
* Generate a lockfile.

At this stage, prioritize correctness over speed.

The initial implementation should support common npm packages and produce useful error messages.

## Phase 2: Global package store

**Objective:** Eliminate repeated downloads and package storage.

Implement:

* Global cache directory.
* Content-addressed package store.
* Tarball cache.
* Integrity verification.
* Atomic package publication.
* Package lookup by name and version.
* Reuse across projects.

This is the phase that most directly implements your original idea.

## Phase 3: Fast filesystem linking

**Objective:** Avoid unnecessary file copying.

Implement:

* Symlink backend.
* Hardlink backend.
* Copy fallback.
* Existing-link detection.
* Incremental linking.
* Store immutability rules.

Benchmark each backend on macOS, Linux, and Windows.

## Phase 4: Efficient lockfile and incremental installation

**Objective:** Make repeated installations fast.

Implement:

* Deterministic dependency graph.
* Frozen lockfile.
* Manifest hashing.
* Installation state.
* No-op installation detection.
* Partial installation recovery.

At this point, Nermo should be able to complete repeated installations without downloading or extracting unchanged packages.

## Phase 5: Parallel installation pipeline

**Objective:** Improve cold installation speed.

Implement:

* Concurrent downloads.
* Concurrent extraction.
* Separate concurrency limits.
* Task scheduling.
* Cancellation.
* Retry policies.
* Progress reporting.

Avoid excessive concurrency and benchmark different configurations.

## Phase 6: Isolated linker and workspaces

**Objective:** Improve dependency isolation and monorepo support.

Implement:

* Isolated dependency layout.
* Peer dependency contexts.
* Workspace resolution.
* Workspace symlinks.
* Hoisted compatibility mode.
* Dependency graph deduplication.

## Phase 7: Advanced optimizations

**Objective:** Reduce remaining overhead.

Potential features:

* Binary metadata cache.
* Optimized archive extraction.
* Background prefetching.
* Store garbage collection.
* Adaptive concurrency.
* Filesystem-specific optimizations.
* Efficient package index.
* Installation profiling.

These should be driven by benchmark evidence.

---

# 26. Features Nermo Should Prioritize

Not every feature has the same impact on your original objective.

The following table ranks implementation priorities by their architectural importance, not by a claim that they will produce a particular speedup.

| Feature                      | Storage reduction | Warm-install benefit | Cold-install benefit | Complexity |
| ---------------------------- | ----------------- | -------------------- | -------------------- | ---------- |
| Global package store         | Very high         | High                 | Medium               | Medium     |
| Content-addressable storage  | Very high         | High                 | Medium               | Medium     |
| Symlink-based installation   | Very high         | Very high            | Medium               | Medium     |
| Hardlink backend             | High              | High                 | Medium               | Medium     |
| Copy-on-write backend        | High              | High                 | Medium               | High       |
| Lockfile                     | Low               | High                 | High                 | Medium     |
| Incremental installation     | Medium            | Very high            | Medium               | High       |
| Parallel downloads           | None              | Low                  | High                 | Medium     |
| Parallel extraction          | None              | Low                  | High                 | High       |
| Binary metadata cache        | None              | Medium               | Medium               | Medium     |
| Isolated linker              | Indirect          | Medium               | Medium               | High       |
| Lifecycle script concurrency | None              | Variable             | Variable             | Medium     |
| Store garbage collection     | High over time    | Indirect             | None                 | Medium     |

These are qualitative assessments. Actual results depend on project size, dependency structure, platform, and cache state.

For Nermo's first public release, I would prioritize:

1. Global package store.
2. Integrity verification.
3. Lockfile support.
4. Incremental installation.
5. Symlink-based linking.
6. Parallel downloads.
7. Safe extraction.
8. Benchmarking.

The other features can be added once the core system is stable.

---

# 27. Important Engineering Trade-offs

Some of Bun's techniques introduce trade-offs that Nermo must address.

## 27.1 Speed versus package isolation

A global store minimizes duplicated data.

However, sharing mutable package directories between projects can cause unexpected behavior.

Nermo should therefore make the store immutable and offer a copy-on-write or copy-based option for workflows that require modifying installed packages.

## 27.2 Speed versus compatibility

An isolated dependency layout can improve dependency correctness.

However, some tools expect a conventional `node_modules` structure.

Nermo should support a compatibility mode and thoroughly test popular JavaScript tooling.

## 27.3 Speed versus freshness

Using cached registry metadata avoids unnecessary network requests.

However, the cache may not contain the newest published version.

For locked installations, this is generally not a problem because the lockfile specifies the required versions.

For commands such as `nermo add package@latest`, Nermo should apply a clear freshness policy.

## 27.4 Speed versus reproducibility

A package manager should not silently select different dependency versions merely to improve installation speed.

Nermo should prioritize reproducibility and integrity.

Performance optimizations should operate on a fixed dependency graph whenever possible.

## 27.5 Speed versus resource consumption

High concurrency can improve performance on some machines.

On others, it can increase memory usage and reduce throughput.

Nermo should use bounded concurrency and make its settings configurable.

---

# 28. What Nermo Can Learn from Bun's Engineering

The most valuable lessons from Bun are architectural rather than language-specific.

### Lesson 1: Treat installation as a systems problem

Package installation is not simply an HTTP download followed by extraction.

It is a complex workload involving networking, parsing, graph traversal, filesystem operations, and process execution.

Nermo should optimize the entire pipeline.

### Lesson 2: Avoid repeating work

The most effective optimization is often eliminating an operation entirely.

If a package already exists in the global store, Nermo should not download or extract it again.

If a project's dependency link is already correct, Nermo should not recreate it.

If the lockfile is unchanged, Nermo should avoid unnecessary dependency resolution.

### Lesson 3: Separate package contents from installation layout

A package's content should not be tightly coupled to where it appears in `node_modules`.

A global store can hold the package once, while multiple projects can expose it through different layouts.

This makes Nermo's storage engine more flexible.

### Lesson 4: Optimize the warm path

Developers frequently reinstall dependencies after switching branches, restoring a project, or deleting `node_modules`.

A fast warm installation can make a significant difference to the development experience.

Nermo should treat warm installation as a first-class workload.

### Lesson 5: Use operating-system capabilities

Hardlinks, symbolic links, atomic renames, and copy-on-write clones can reduce unnecessary work.

Nermo should use the best available filesystem mechanism while retaining safe fallbacks.

### Lesson 6: Keep performance measurable

Without profiling, it is easy to optimize the wrong part of the installation process.

Nermo should include performance instrumentation early in development.

---

# 29. Final Recommended Design for Nermo

The central idea behind Nermo should be:

**Download once, verify once, store once, and reuse everywhere.**

A project should not need a separate physical copy of every dependency when the same package content is already available in the developer's global store.

The proposed architecture is:

```text
                  NERMO
                    |
          +---------+---------+
          |                   |
          v                   v
    Dependency Engine    Global Store
          |                   |
          v                   v
       Lockfile         Content-Addressed
          |                   |
          v                   v
   Installation Plan    Verified Packages
          |                   |
          +---------+---------+
                    |
                    v
              Linker Engine
                    |
          +---------+---------+
          |                   |
          v                   v
     Isolated Mode       Hoisted Mode
          |                   |
          +---------+---------+
                    |
                    v
              node_modules
```

The package manager should use a global content-addressed store, a deterministic lockfile, and an installation planner that minimizes unnecessary work.

It should then use the most appropriate filesystem backend for the operating system.

The result would be a package manager that combines the storage benefits of a shared cache with the familiar Node.js dependency layout.

## 29.1 Proposed initial performance goals

These are engineering targets for Nermo, not claims about existing performance.

| Metric                      | Initial target                        |
| --------------------------- | ------------------------------------- |
| Repeated no-op installation | Avoid network and extraction          |
| Warm installation           | Link only missing dependencies        |
| Repeated package downloads  | Zero for verified cached packages     |
| Repeated package extraction | Zero for valid cached packages        |
| Package integrity           | Verify every newly downloaded package |
| Offline installation        | Supported with a complete cache       |
| Lockfile installation       | Deterministic                         |
| Package store               | Shared across projects                |
| Filesystem backend          | Platform-aware                        |
| Installation concurrency    | Configurable                          |
| Recovery                    | No partially published packages       |

These goals should be converted into automated tests and benchmark fixtures.

## 29.2 The most important architectural decision

If there is one feature I would prioritize above all the other performance optimizations, it is the **global immutable package store combined with a fast linker**.

This is particularly aligned with your original goal of reducing disk usage across multiple projects.

It also gives Nermo a strong foundation for future features:

* Offline installations.
* Shared caches.
* Fast project restoration.
* Monorepo support.
* CI caching.
* Dependency prefetching.
* Store garbage collection.

A fast downloader is useful, but a package manager that avoids downloading, extracting, and copying the same dependency repeatedly can save work across an entire development environment.

---

# 30. Research Sources and Further Reading

The following official Bun resources provide the most relevant technical information for implementing Nermo.

1. **[Bun: Behind the Scenes of Bun Install](https://bun.com/blog/behind-the-scenes-of-bun-install?utm_source=chatgpt.com)**
   Bun's engineering article discussing its installation architecture, filesystem optimizations, metadata caching, extraction, and concurrency.

2. **[Bun: Global Package Cache](https://bun.sh/docs/pm/global-cache?utm_source=chatgpt.com)**
   Documentation explaining Bun's global cache, package reuse, filesystem backends, and storage behavior.

3. **[Bun: Global Virtual Store](https://bun.com/docs/pm/global-store?utm_source=chatgpt.com)**
   Documentation on sharing package contents between projects through a global virtual store and symlinks.

4. **[Bun: Install Command](https://bun.com/docs/pm/cli/install?utm_source=chatgpt.com)**
   Reference for installation behavior, lifecycle scripts, caching, installation strategies, and platform-specific dependencies.

5. **[Bun: Lockfile](https://bun.com/docs/pm/lockfile?utm_source=chatgpt.com)**
   Documentation covering Bun's lockfile format, migration, and reproducible installations.

6. **[Bun: Isolated Installs](https://bun.com/docs/pm/isolated-installs?utm_source=chatgpt.com)**
   Technical documentation explaining dependency isolation, installation layouts, and the global virtual store.

---

# Conclusion

Bun's installation speed is not attributable to a single algorithm or optimization. It results from a collection of engineering decisions that reduce unnecessary work across the entire package installation pipeline.

For Nermo, the most relevant combination is:

* A native Rust implementation.
* A global content-addressed package store.
* An efficient lockfile and dependency resolver.
* Incremental installation.
* Concurrent downloads and extraction.
* Fast filesystem linking.
* Efficient metadata caching.
* Safe and atomic package publication.

The central design principle is to **reuse verified package contents and avoid repeating expensive operations**.

Nermo does not need to reproduce Bun's internal implementation exactly. It can adopt the same general principles while designing its own storage model around your specific goal: making npm dependencies reusable across local projects without sacrificing Node.js compatibility.

The first milestone should be a working Rust package manager that can install dependencies into a shared global store and link them into a project. Once that works reliably, the next step should be benchmarking the warm-install path and progressively optimizing the operations that consume the most time.

[1]: https://bun.com/docs/pm/cli/install?utm_source=chatgpt.com "bun install | Bun Docs"
[2]: https://bun.sh/docs/pm/global-cache?utm_source=chatgpt.com "Global cache | Bun Docs"
[3]: https://bun.com/docs/pm/global-store?utm_source=chatgpt.com "Global virtual store | Bun Docs"
[4]: https://bun.com/docs/pm/lockfile?utm_source=chatgpt.com "Lockfile | Bun Docs"
[5]: https://bun.com/blog/behind-the-scenes-of-bun-install?utm_source=chatgpt.com "Behind The Scenes of Bun Install | Bun Blog"
[6]: https://bun.sh/docs/pm/cli/install?utm_source=chatgpt.com "bun install | Bun Docs"
[7]: https://bun.com/docs/pm/isolated-installs?utm_source=chatgpt.com "Isolated installs | Bun Docs"
