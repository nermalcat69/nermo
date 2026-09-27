# NERMO

## Product Requirements Document (PRD)

**Product:** Nermo
**Category:** JavaScript Package Manager / Local Dependency Store
**Language:** Rust
**Target ecosystem:** Node.js and npm
**Platform:** macOS, Linux, Windows
**Version:** 1.0 — Initial Product Specification
**Status:** Proposed
**Document type:** Product and Technical Requirements

---

# 1. Executive Summary

## 1.1 Overview

Nermo is a lightweight, high-performance package manager written in Rust, designed to eliminate redundant dependency storage across JavaScript and TypeScript projects.

Modern JavaScript development frequently involves working on multiple projects that share hundreds of identical dependencies. Traditional package managers install dependencies into individual project directories, resulting in duplicated files and unnecessary disk consumption.

Nermo addresses this problem by introducing a centralized, persistent package store that can be shared across multiple projects on the same machine.

Instead of downloading and extracting identical packages repeatedly, Nermo stores packages in a global directory and links them into individual projects.

Nermo is designed primarily for local development. It should not require deployment providers, CI environments, or production servers to recognize or install Nermo.

Projects managed locally with Nermo should remain compatible with the existing npm ecosystem.

Developers should be able to switch between Nermo, npm, pnpm, and Bun without having to restructure their projects.

## 1.2 Vision

Build a fast, minimal, and storage-efficient package manager that makes managing JavaScript dependencies across multiple projects substantially more efficient.

Nermo should make dependency installation nearly instantaneous when packages are already available locally, while minimizing disk usage and avoiding unnecessary filesystem operations.

The product should prioritize:

* Fast dependency installation.
* Efficient global package storage.
* Minimal disk consumption.
* Compatibility with the existing npm ecosystem.
* Simple local development workflows.
* Reproducible installations.
* Minimal interference with deployment environments.
* A small, maintainable Rust codebase.

## 1.3 Core Concept

Nermo separates package storage from project dependencies.

Instead of maintaining independent copies of packages in every project, Nermo maintains a global store and exposes packages to projects through filesystem links.

Example:

A developer has five projects:

```text
~/Projects/
├── website/
├── dashboard/
├── ecommerce/
├── documentation/
└── admin-panel/
```

All five projects depend on React.

With traditional npm installations, each project can contain its own copy of React.

With Nermo:

```text
~/.nermo/
└── store/
    └── packages/
        └── react/
            └── 19.0.0/
```

Each project can reference the same stored package:

```text
website/node_modules/react
    -> ~/.nermo/store/packages/react/19.0.0

dashboard/node_modules/react
    -> ~/.nermo/store/packages/react/19.0.0

ecommerce/node_modules/react
    -> ~/.nermo/store/packages/react/19.0.0

documentation/node_modules/react
    -> ~/.nermo/store/packages/react/19.0.0

admin-panel/node_modules/react
    -> ~/.nermo/store/packages/react/19.0.0
```

Only one physical copy of React is required.

The same principle applies to other dependencies, including Next.js, TypeScript, ESLint, Vite and thousands of other npm packages.

Nermo should also support dependency version conflicts by allowing multiple versions of the same package to coexist in the global store.

## 1.4 Product Positioning

Nermo is not intended to replace the Node.js runtime, JavaScript bundlers, or the npm registry.

It is a package installation and storage system optimized for local development.

Its architectural inspiration includes:

* npm's package manifest and registry compatibility.
* pnpm's global content-addressable store and symlink-based dependency layout.
* Bun's focus on installation speed and developer experience.

Nermo's distinguishing objective is to make a shared package store a first-class part of the local development workflow while keeping deployment independent of Nermo.

---

# 2. Problem Statement

## 2.1 Dependency Duplication

JavaScript projects commonly share many dependencies.

For example, a developer working on several Next.js applications might have the following dependency trees:

```text
Project A
├── next
├── react
├── react-dom
├── typescript
├── eslint
└── zod

Project B
├── next
├── react
├── react-dom
├── typescript
├── eslint
└── zod

Project C
├── next
├── react
├── react-dom
├── typescript
├── eslint
└── zod
```

Although these projects use identical package versions, conventional installations can create separate copies of the same files.

This results in:

* Increased disk usage.
* Repeated package extraction.
* Unnecessary filesystem operations.
* Longer installation times.
* More time spent waiting for dependencies during development.

The problem becomes more pronounced when developers work on many repositories simultaneously.

## 2.2 Inefficient Reinstallation

Developers frequently delete `node_modules`, clone repositories, switch branches, or reinstall dependencies.

Even when most dependencies are already present on the machine, conventional package managers may need to recreate project-specific dependency directories.

Nermo should eliminate unnecessary downloads and package extraction whenever possible.

## 2.3 Deployment Independence

A package manager that modifies project configuration or introduces proprietary dependency formats can complicate deployment.

Developers should not have to configure their hosting provider to recognize Nermo.

Nermo should therefore maintain compatibility with conventional JavaScript project structures.

The project's `package.json` should remain the primary dependency manifest.

Nermo-specific metadata should be optional and separate from the standard npm manifest.

## 2.4 Developer Experience

Package managers are frequently used through terminal commands.

Nermo should provide a familiar interface, allowing developers to adopt it without learning a completely different workflow.

Its commands should resemble established package managers wherever practical.

---

# 3. Product Goals

## 3.1 Primary Goals

### G1. Centralized Package Storage

Provide a global package store shared across projects belonging to the same user.

The store should avoid downloading or extracting a package again when a verified copy already exists.

### G2. Efficient Local Linking

Expose stored packages to projects through filesystem links.

The resulting project structure must remain compatible with Node.js module resolution.

### G3. High Installation Performance

Optimize both initial and repeated installations.

The most important performance objective is minimizing the time required to install dependencies when the global store is already populated.

### G4. Standard npm Compatibility

Support standard `package.json` manifests and the npm registry.

Projects should remain usable with npm and other compatible package managers.

### G5. Deployment Independence

Nermo should not be required during production deployment.

Projects should be deployable through conventional package managers without Nermo-specific configuration.

### G6. Reproducible Installations

Provide a Nermo-specific lockfile that records the resolved dependency graph and package integrity information.

Support deterministic installation from that lockfile.

### G7. Minimal Resource Consumption

Minimize unnecessary disk writes, memory usage, CPU utilization and network traffic.

### G8. Cross-Platform Support

Support macOS, Linux and Windows, with platform-specific linking strategies where necessary.

## 3.2 Secondary Goals

* Provide package-store inspection and maintenance commands.
* Support workspace projects.
* Support offline installations when all required packages are cached.
* Provide installation progress and diagnostic output.
* Support package aliases and scoped npm packages.
* Offer configurable package-store locations.
* Provide an optional global cache cleanup mechanism.

---

# 4. Non-Goals

The initial versions of Nermo will not attempt to:

1. Replace the Node.js runtime.
2. Implement a JavaScript bundler.
3. Implement a TypeScript compiler.
4. Host an independent package registry.
5. Replace npm's publishing infrastructure.
6. Support every npm package lifecycle script from the first release.
7. Provide a cloud-based package store.
8. Automatically synchronize dependencies between machines.
9. Replace deployment providers' package installation workflows.
10. Guarantee compatibility with every existing npm package in the MVP.

Nermo should prioritize a reliable core installation system over extensive feature coverage.

---

# 5. Target Users

## 5.1 Primary Persona: JavaScript Developer

A developer working on multiple React, Next.js, Vue, Svelte or Node.js projects.

Typical characteristics:

* Uses macOS or Linux.
* Has multiple repositories on one machine.
* Frequently reinstalls dependencies.
* Uses npm, pnpm or Bun.
* Wants to reduce disk consumption.
* Values fast development workflows.

Primary use case:

Install dependencies in multiple projects without maintaining redundant copies of the same packages.

## 5.2 Secondary Persona: Full-Stack Developer

A developer working with frontend and backend JavaScript applications.

Typical dependencies include:

* React.
* Next.js.
* Express.
* Fastify.
* TypeScript.
* Prisma.
* Drizzle.
* ESLint.
* Prettier.

Primary use case:

Use a single global package store across multiple applications, including frontend applications, APIs and shared packages.

## 5.3 Secondary Persona: Developer Working With Many Repositories

A developer maintaining open-source projects, monorepos, client applications or multiple products.

Primary use case:

Reduce repeated installations and disk usage across numerous repositories.

## 5.4 Secondary Persona: CI and Build Engineers

Although local development is the primary focus, Nermo may eventually support CI environments.

Potential use cases include:

* Reusing a persistent dependency cache.
* Avoiding repeated downloads.
* Installing dependencies in isolated build environments.

CI support is not a requirement for the initial release.

---

# 6. Product Principles

Nermo should follow these principles throughout development.

### 6.1 Local First

Nermo should work without requiring an account, hosted service or remote synchronization system.

All package storage should be local by default.

### 6.2 Immutable Package Storage

Once a package has been downloaded, verified and stored, its contents should not be modified.

A different package version or different package content should receive a separate store entry.

This reduces the risk of projects accidentally modifying shared dependencies.

### 6.3 Standard Project Structure

Projects should retain a conventional `package.json` and `node_modules` directory.

Developers should be able to open projects in existing editors and run their existing scripts.

### 6.4 Deployment Neutrality

Nermo-specific configuration must not be required for production builds.

A deployment provider should be able to use npm or another compatible package manager.

### 6.5 Performance Through Avoidance

The primary optimization should be eliminating unnecessary work rather than simply making every operation faster.

Nermo should avoid:

* Redownloading cached packages.
* Re-extracting existing packages.
* Recreating unchanged links.
* Rewriting unchanged lockfiles.
* Re-resolving an unchanged dependency graph unnecessarily.

### 6.6 Explicit Behavior

Nermo should not silently modify project dependencies, execute unexpected commands or remove files outside its managed directories.

---

# 7. System Architecture

## 7.1 High-Level Architecture

Nermo will be implemented in Rust as a modular command-line application.

The architecture will consist of the following components:

```text
                     NERMO CLI
                         |
          +--------------+--------------+
          |              |              |
       Commands       Resolver        Config
          |              |              |
          |         Dependency Graph    |
          |              |              |
          +--------------+--------------+
                         |
                 Installation Engine
                         |
          +--------------+--------------+
          |              |              |
      Registry        Global Store     Lockfile
       Client          Manager         Manager
          |              |              |
          +--------------+--------------+
                         |
                    Package Linker
                         |
                    node_modules
```

## 7.2 CLI

The CLI is the main user interface.

Responsibilities:

* Parse commands and arguments.
* Discover the current project.
* Load configuration.
* Display progress and errors.
* Invoke installation operations.
* Provide machine-readable output when requested.

The CLI should remain independent of the package resolver and storage implementation.

## 7.3 Registry Client

The registry client communicates with npm-compatible registries.

Responsibilities:

* Retrieve package metadata.
* Retrieve version metadata.
* Download package tarballs.
* Handle HTTP redirects.
* Support registry authentication.
* Respect registry configuration.
* Verify package integrity.

The initial implementation should target the public npm registry while allowing custom registries through configuration.

## 7.4 Dependency Resolver

The resolver constructs the complete dependency graph.

Responsibilities:

* Parse dependency specifications.
* Resolve semantic version ranges.
* Resolve transitive dependencies.
* Handle dependency aliases.
* Identify compatible package versions.
* Detect dependency conflicts.
* Generate deterministic resolution output.
* Produce the graph used by the linker.

The resolver must distinguish between a package's identity and its installation location.

For example, two packages may depend on different versions of the same dependency.

Both versions must coexist without breaking Node.js module resolution.

## 7.5 Global Store Manager

The global store manager owns all downloaded and extracted packages.

Responsibilities:

* Determine package storage paths.
* Check whether a package already exists.
* Verify stored package integrity.
* Download missing packages.
* Extract package archives.
* Maintain package metadata.
* Prevent concurrent installation conflicts.
* Identify unreferenced packages.
* Support offline operation.

The global store should be independent of individual project directories.

## 7.6 Linker

The linker connects packages in the global store to project-level `node_modules` directories.

Responsibilities:

* Create package links.
* Preserve dependency resolution.
* Handle scoped packages.
* Handle conflicting versions.
* Create nested dependency links where necessary.
* Detect stale links.
* Remove obsolete managed links.
* Preserve unmanaged project files.

The linker must never assume that all packages can be placed at the top level.

## 7.7 Lockfile Manager

The lockfile manager reads and writes Nermo's local lockfile.

Responsibilities:

* Record resolved versions.
* Record dependency relationships.
* Record registry and integrity information.
* Record package store identifiers.
* Preserve deterministic dependency resolution.
* Support frozen installation.
* Detect incompatible lockfile versions.

---

# 8. Global Package Store

## 8.1 Default Location

Nermo should use a user-specific application data directory.

Default locations:

**macOS**

```text
~/Library/Application Support/nermo/
```

**Linux**

```text
~/.local/share/nermo/
```

**Windows**

```text
%LOCALAPPDATA%\Nermo\
```

The exact location should follow the operating system's conventions.

Users should be able to override the location using configuration or an environment variable.

## 8.2 Proposed Directory Structure

```text
nermo/
├── config.toml
├── store/
│   ├── packages/
│   │   ├── react/
│   │   │   ├── 18.3.1/
│   │   │   └── 19.0.0/
│   │   ├── next/
│   │   │   └── 15.5.0/
│   │   └── typescript/
│   │       └── 5.8.0/
│   ├── metadata/
│   └── temporary/
├── cache/
│   ├── registry/
│   └── resolution/
├── projects/
└── logs/
```

This structure represents the initial logical layout. The physical store should be abstracted so that it can support content-addressable storage later.

## 8.3 Package Identity

A package installation should be identified using sufficient information to distinguish different package contents.

At a minimum:

* Package name.
* Package version.
* Registry.
* Package integrity hash.

Package identity must not rely solely on the package name and version.

A registry may serve different content for the same name and version, and integrity verification must detect this.

## 8.4 Content-Addressable Storage

The architecture should support a future content-addressable store.

A package archive's integrity hash can be used to verify the downloaded tarball.

An additional normalized content hash may be used to identify identical extracted package contents.

For example:

```text
~/.nermo/store/
└── sha512/
    ├── 1a2b3c.../
    ├── 4d5e6f.../
    └── 7a8b9c.../
```

The store should maintain metadata mapping package identities to stored content.

Content-addressable storage is a planned architectural capability. The MVP may use package-name-and-version directories to reduce implementation complexity.

## 8.5 Immutability

Packages in the global store must be treated as immutable after installation.

The package manager must not permit ordinary project operations to modify shared package contents.

If a package is modified externally, Nermo should detect the inconsistency when verification is requested.

The MVP should not attempt to repair modified packages automatically without user consent.

## 8.6 Concurrent Access

Multiple Nermo processes may attempt to install the same package.

The store must prevent duplicate extraction and partial package visibility.

Recommended approach:

1. Check whether the package already exists.
2. Acquire a package-specific lock.
3. Recheck the package after acquiring the lock.
4. Download the package if necessary.
5. Extract it into a temporary directory.
6. Verify the extracted contents.
7. Atomically move the completed package into the store.
8. Release the lock.

Temporary files should be cleaned up after failed installations.

---

# 9. Project Structure and Deployment Compatibility

## 9.1 Standard Project Layout

A Nermo-managed project should look like a conventional npm project.

```text
my-project/
├── package.json
├── .nermo-lock
├── node_modules/
│   ├── react/
│   ├── next/
│   └── typescript/
├── src/
└── ...
```

The `.nermo-lock` file is Nermo-specific.

The `package.json` remains the authoritative dependency manifest.

## 9.2 No Mandatory package.json Changes

Nermo must not require proprietary fields in `package.json`.

It should work with ordinary dependency declarations.

Example:

```json
{
  "name": "my-project",
  "version": "1.0.0",
  "scripts": {
    "dev": "next dev",
    "build": "next build"
  },
  "dependencies": {
    "next": "^15.5.0",
    "react": "^19.0.0",
    "react-dom": "^19.0.0"
  },
  "devDependencies": {
    "typescript": "^5.8.0"
  }
}
```

Nermo should read and interpret this manifest without requiring changes.

## 9.3 Deployment Workflow

Nermo is intended to be a local development tool.

A typical workflow:

```bash
# Local development
nermo install

# Run development server
npm run dev

# Commit project source
git add .
git commit -m "Add application"

# Deploy using a conventional provider
```

The deployment provider should install dependencies from the standard manifest and its supported lockfile.

Nermo's lockfile should not be required for deployment.

## 9.4 Git Integration

Nermo should provide an optional command to configure `.gitignore`.

Example:

```gitignore
# Nermo
.nermo-lock
```

The `node_modules` directory should also be ignored, as is standard practice for JavaScript projects.

Nermo should never silently remove an existing lockfile belonging to npm, pnpm or Bun.

Users should be able to choose whether `.nermo-lock` is committed.

## 9.5 Existing Lockfiles

Nermo must detect existing package-manager lockfiles.

Recognized files include:

* `package-lock.json`
* `pnpm-lock.yaml`
* `bun.lock`
* `bun.lockb`
* `yarn.lock`

The MVP should use `package.json` as its dependency input.

Future versions may import resolutions from other package-manager lockfiles.

Nermo must clearly communicate when an existing lockfile is not being used.

It must not claim reproducibility when dependency versions are being resolved from ranges without a preserved resolution.

---

# 10. Dependency Resolution

## 10.1 Requirements

The dependency resolver is one of the most complex components of Nermo.

It must support:

* Exact versions.
* Semantic version ranges.
* Caret ranges.
* Tilde ranges.
* Prerelease versions.
* Scoped packages.
* Transitive dependencies.
* Multiple versions of the same package.
* Peer dependencies.
* Optional dependencies.
* Dependency aliases.

The initial MVP may support a smaller subset, but the resolver interface should accommodate the complete dependency graph.

## 10.2 Example Dependency Graph

Consider the following project:

```json
{
  "dependencies": {
    "package-a": "^1.0.0",
    "package-b": "^2.0.0"
  }
}
```

Suppose:

```text
package-a@1.2.0
└── lodash@4.17.21

package-b@2.1.0
└── lodash@3.10.1
```

The resolver must preserve both versions.

The resulting dependency graph is:

```text
project
├── package-a@1.2.0
│   └── lodash@4.17.21
└── package-b@2.1.0
    └── lodash@3.10.1
```

The linker must ensure each package resolves its intended version.

## 10.3 Resolution Strategy

The resolver should use a deterministic algorithm.

For each dependency:

1. Parse the dependency specification.
2. Check whether an existing lockfile resolution can be reused.
3. Retrieve metadata if resolution is necessary.
4. Identify versions satisfying the requested range.
5. Select a compatible version.
6. Resolve its dependencies recursively.
7. Detect conflicts and invalid dependency cycles.
8. Construct the complete dependency graph.
9. Produce a stable lockfile representation.

The resolver should avoid unnecessary network requests by caching registry metadata.

## 10.4 Resolution Caching

The resolver should cache package metadata and previous resolution results.

A cached resolution may be reused only when the relevant inputs remain unchanged.

Inputs should include:

* Package name.
* Version range.
* Registry.
* Relevant dependency configuration.
* Lockfile state.

Cached data must not override explicit user requests for fresh resolution.

---

# 11. Package Linking

## 11.1 Linking Strategy

Nermo should use filesystem links to expose global packages inside project directories.

The preferred implementation is symbolic links where supported.

The package manager should provide a platform abstraction to support alternative linking methods.

## 11.2 Basic Linking

Example global store:

```text
~/.nermo/store/packages/
├── react/19.0.0/
├── next/15.5.0/
└── typescript/5.8.0/
```

Project directory:

```text
project/
└── node_modules/
    ├── react -> ~/.nermo/store/packages/react/19.0.0
    ├── next -> ~/.nermo/store/packages/next/15.5.0
    └── typescript -> ~/.nermo/store/packages/typescript/5.8.0
```

Node.js should be able to resolve these packages using its standard module-resolution algorithm.

## 11.3 Nested Dependencies

Nested dependencies must be supported.

For example:

```text
node_modules/
├── package-a/
│   └── node_modules/
│       └── lodash -> global-store/lodash/4.17.21
└── package-b/
    └── node_modules/
        └── lodash -> global-store/lodash/3.10.1
```

Nermo should use the minimum number of links necessary while preserving dependency resolution.

## 11.4 Scoped Packages

Scoped packages must follow the standard directory layout.

Example:

```text
node_modules/
└── @types/
    ├── node -> global-store/@types/node/22.0.0
    └── react -> global-store/@types/react/19.0.0
```

The linker must create scope directories as needed.

## 11.5 Existing node_modules

Nermo must handle projects that already contain `node_modules`.

It should inspect existing entries before modifying them.

By default, it should only replace links and files it can identify as Nermo-managed.

If an unmanaged directory conflicts with an intended link, Nermo should report the conflict and request an explicit action.

## 11.6 Windows Compatibility

Windows does not have identical symlink permissions and behavior to Unix-like systems.

Nermo should provide a Windows-specific linking backend.

Potential strategies include:

* Directory symbolic links.
* Junctions.
* Other supported filesystem link mechanisms.

The implementation should select an appropriate method based on platform capabilities and permissions.

---

# 12. Lockfile Specification

## 12.1 Purpose

The Nermo lockfile records the exact dependency graph selected for a project.

It allows subsequent installations to reproduce the same dependency resolution.

## 12.2 Filename

The proposed filename is:

```text
.nermo-lock
```

The file should be JSON in the initial release for ease of debugging and implementation.

## 12.3 Example

```json
{
  "lockfileVersion": 1,
  "project": {
    "name": "example-project",
    "version": "1.0.0"
  },
  "dependencies": {
    "react": {
      "version": "19.0.0",
      "resolved": "https://registry.npmjs.org/react/-/react-19.0.0.tgz",
      "integrity": "sha512-...",
      "storeKey": "sha512/abc123"
    },
    "typescript": {
      "version": "5.8.0",
      "resolved": "https://registry.npmjs.org/typescript/-/typescript-5.8.0.tgz",
      "integrity": "sha512-...",
      "storeKey": "sha512/def456"
    }
  }
}
```

This is an illustrative schema. The production schema must also represent nested dependencies, peer dependencies, optional dependencies and platform-specific conditions.

## 12.4 Lockfile Requirements

The lockfile must:

* Include a schema version.
* Preserve exact resolved package versions.
* Record integrity information.
* Record package source information.
* Represent dependency relationships.
* Support deterministic serialization.
* Support migration between schema versions.
* Be written atomically.

## 12.5 Frozen Installations

Command:

```bash
nermo install --frozen
```

Behavior:

* Require a compatible lockfile.
* Do not update package resolutions.
* Fail if the manifest and lockfile are incompatible.
* Download missing packages if networking is available.
* Reuse verified packages from the global store.
* Create the required project links.

Frozen installation should be suitable for reproducible local development.

---

# 13. CLI Requirements

## 13.1 Command Overview

The initial command set should include:

```text
nermo install
nermo add
nermo remove
nermo update
nermo store
nermo store prune
nermo clean
nermo doctor
nermo --version
nermo --help
```

## 13.2 `nermo install`

Purpose: Install all dependencies declared in the current project's `package.json`.

Usage:

```bash
nermo install
```

Requirements:

* Discover the project root.
* Read `package.json`.
* Load the lockfile if present.
* Resolve dependencies when necessary.
* Check the global store.
* Download missing packages.
* Verify package integrity.
* Create or update links.
* Write the lockfile.
* Display installation results.

Expected output:

```text
nermo install

Resolving dependencies...
Found 42 packages.

Checking global store...
Already available: 38
Missing: 4

Downloading packages...
Extracting packages...
Linking dependencies...

Installation completed.

Packages: 42
Downloaded: 4
Reused: 38
Duration: 0.84s
```

The exact timing will depend on the workload and hardware.

## 13.3 `nermo add`

Purpose: Add a dependency to the current project.

Examples:

```bash
nermo add react
nermo add react@19
nermo add zod
nermo add -D typescript
```

Requirements:

* Resolve the requested package.
* Update `package.json`.
* Install the dependency.
* Update the lockfile.
* Create the appropriate links.

The command must preserve unrelated manifest fields.

## 13.4 `nermo remove`

Purpose: Remove a dependency.

Example:

```bash
nermo remove react
```

Requirements:

* Remove the dependency from the appropriate manifest section.
* Update the dependency graph.
* Remove obsolete project links.
* Preserve packages still needed by other dependencies.
* Update the lockfile.

Removing a package from a project must not automatically remove it from the global store.

## 13.5 `nermo update`

Purpose: Update dependencies according to their declared version ranges.

Usage:

```bash
nermo update
nermo update react
```

The command should update selected resolutions and regenerate the lockfile.

It should not silently change the version ranges declared in `package.json` unless explicitly requested.

## 13.6 `nermo store`

Purpose: Inspect the global package store.

Usage:

```bash
nermo store
```

Example output:

```text
Nermo Store

Location:
~/.local/share/nermo/store

Packages:          1,284
Package versions:  1,592
Disk usage:        2.4 GB

Projects tracked:  18
```

The implementation should calculate actual store statistics rather than display fixed values.

## 13.7 `nermo store prune`

Purpose: Remove unused packages from the global store.

Usage:

```bash
nermo store prune
```

Requirements:

* Identify packages referenced by tracked projects.
* Identify unreferenced packages.
* Display the proposed cleanup.
* Require confirmation before deletion by default.
* Support a dry-run mode.
* Avoid deleting packages currently used by active installations.

Example:

```text
Nermo Store Cleanup

Unused packages: 126
Potential space recovery: 840 MB

Proceed? [y/N]
```

The initial implementation may use project lockfiles to determine references.

Because projects may not have been opened recently, pruning should be conservative.

## 13.8 `nermo clean`

Purpose: Remove Nermo-managed project links.

Usage:

```bash
nermo clean
```

The command must not delete user files or unmanaged dependencies.

## 13.9 `nermo doctor`

Purpose: Diagnose common configuration and installation problems.

Checks should include:

* Rust-independent Nermo installation health.
* Store accessibility.
* Registry connectivity.
* Package integrity.
* Project configuration.
* Broken symlinks.
* Lockfile compatibility.
* Filesystem permissions.

---

# 14. Installation Engine

## 14.1 Overview

The installation engine coordinates dependency resolution, package retrieval, verification, storage and linking.

It should be designed as a transaction-like process.

## 14.2 Installation Pipeline

```text
Start
  |
  v
Discover Project
  |
  v
Read package.json
  |
  v
Read Lockfile
  |
  v
Resolve Dependencies
  |
  v
Check Global Store
  |
  +---- Packages Missing? ----+
  |                           |
 Yes                          No
  |                           |
  v                           |
Download Packages             |
  |                           |
  v                           |
Verify Integrity              |
  |                           |
  v                           |
Extract to Temporary Store    |
  |                           |
  v                           |
Commit Packages               |
  |                           |
  +-------------+-------------+
                |
                v
         Build Link Plan
                |
                v
         Create/Update Links
                |
                v
         Write Lockfile
                |
                v
         Installation Complete
```

## 14.3 Failure Handling

Nermo must handle:

* Network interruptions.
* Registry errors.
* Invalid package archives.
* Integrity mismatches.
* Insufficient disk space.
* Permission errors.
* Invalid manifests.
* Dependency resolution failures.
* Broken links.
* Concurrent installations.

Where possible, failed installations should leave the previous working installation intact.

## 14.4 Atomic Operations

Package extraction should occur in a temporary directory.

The completed package should only become visible in the global store after verification.

Lockfile writes should use temporary files and atomic replacement where supported.

Project link updates should use a planned operation sequence with rollback where feasible.

Full atomicity across all filesystem operations may not be possible on every platform, so the implementation must document its recovery behavior.

---

# 15. Performance Requirements

## 15.1 Performance Philosophy

Nermo's primary performance advantage should come from avoiding repeated work.

The most important operations to optimize are:

1. Warm installations.
2. Installation across multiple projects.
3. Dependency graph reuse.
4. Package lookup.
5. Link creation.
6. Package verification.

## 15.2 Performance Targets

The following are proposed engineering targets, not established benchmarks.

| Operation                             | Initial target                                         |
| ------------------------------------- | ------------------------------------------------------ |
| CLI startup                           | Under 100 ms on typical development hardware           |
| Warm installation of a small project  | Under 500 ms                                           |
| Warm installation of a medium project | Under 2 seconds                                        |
| Cached package lookup                 | Under 10 ms per package                                |
| Unchanged lockfile generation         | No unnecessary rewrite                                 |
| Repeated package download             | Zero network requests when valid cached content exists |
| Duplicate package storage             | One stored copy per identical content hash             |

These targets should be evaluated against a fixed benchmark environment.

The first release should prioritize correctness over meeting every target.

## 15.3 Installation Benchmarks

Nermo must include reproducible benchmarks for:

### Cold Installation

Install a project with an empty store.

Measure:

* Total installation time.
* Network transfer time.
* Extraction time.
* Resolution time.
* Link creation time.
* Peak memory consumption.

### Warm Installation

Install the same project with all packages already cached.

Measure:

* Dependency resolution time.
* Store lookup time.
* Link creation time.
* Lockfile processing time.
* Total installation time.

### Multi-Project Installation

Install dependencies across 10 or more projects sharing packages.

Measure:

* Total time.
* Total disk usage.
* Number of package downloads.
* Number of extracted packages.
* Number of links created.

### Incremental Installation

Modify a project's dependencies and reinstall.

Measure how much work is performed for unchanged dependencies.

## 15.4 Benchmark Comparison

Nermo should be benchmarked against:

* npm.
* pnpm.
* Bun.

All package managers should use the same machine, registry, project manifests and network conditions.

The benchmark report should distinguish between cold and warm installations.

No performance claims should be published without reproducible test results.

---

# 16. Security Requirements

## 16.1 Package Integrity

Nermo must verify downloaded packages against the integrity information supplied by the registry or lockfile.

Packages that fail verification must not be installed.

## 16.2 Archive Extraction

The archive extractor must defend against path traversal.

It must reject archive entries that attempt to write outside the intended extraction directory.

Symbolic links contained in archives must be handled carefully.

## 16.3 Lifecycle Scripts

npm packages may contain lifecycle scripts that execute during installation.

Nermo should not automatically execute arbitrary scripts without an explicit, documented policy.

The MVP should provide a clear installation policy and an option for users to control script execution.

## 16.4 Registry Authentication

Nermo should support standard npm registry authentication configuration where practical.

Credentials must not be printed in logs.

## 16.5 Global Store Isolation

The global store should be writable only by the appropriate user.

Nermo must not assume that packages in a shared store are trustworthy merely because they are already present.

## 16.6 Symlink Safety

The linker must validate target paths and avoid creating links that escape the intended store.

Cleanup operations must not follow arbitrary symlinks and delete external directories.

---

# 17. Configuration

## 17.1 Configuration File

Nermo should support a global TOML configuration file.

Example:

```toml
[store]
path = "~/.local/share/nermo/store"

[registry]
default = "https://registry.npmjs.org/"

[install]
concurrency = 16
verify_integrity = true
run_scripts = false

[linker]
strategy = "auto"

[performance]
resolution_cache = true
incremental_install = true
```

This is a proposed configuration schema.

## 17.2 Project Configuration

Project-specific configuration should be optional.

A project may contain:

```text
.nermo/
└── config.toml
```

Possible settings:

* Registry overrides.
* Linker preferences.
* Installation behavior.
* Script execution policy.
* Workspace configuration.

Project configuration must not be required for ordinary installations.

## 17.3 Environment Variables

Nermo should support environment variables for common configuration options.

Examples:

```bash
NERMO_HOME
NERMO_STORE
NERMO_REGISTRY
NERMO_CONCURRENCY
```

Environment variables should take precedence over global configuration.

---

# 18. Offline Mode

## 18.1 Requirements

Nermo should support installation without network access when all required packages and metadata are available locally.

Command:

```bash
nermo install --offline
```

Behavior:

* Do not contact the registry.
* Use the existing lockfile.
* Check the global store.
* Fail if a required package is missing.
* Create project links when all required packages are available.

## 18.2 Offline Diagnostics

When an offline installation cannot complete, Nermo should report the missing packages.

Example:

```text
Offline installation failed.

Missing packages:
- package-a@1.2.0
- package-b@3.0.0

Connect to the registry and run:
nermo install
```

---

# 19. Workspace Support

Workspace support is an important future capability.

Many JavaScript repositories use npm, pnpm or Yarn workspaces.

Example:

```text
monorepo/
├── package.json
├── packages/
│   ├── web/
│   │   └── package.json
│   ├── api/
│   │   └── package.json
│   └── shared/
│       └── package.json
└── apps/
    └── dashboard/
        └── package.json
```

Nermo should eventually support:

* Workspace discovery.
* Workspace dependency references.
* Workspace package linking.
* Shared dependency resolution.
* Workspace-aware lockfiles.
* Workspace-level installation.

The first MVP may support only single-package projects.

Workspace support should be introduced after the core dependency resolver and linker are stable.

---

# 20. Compatibility Requirements

## 20.1 Node.js Compatibility

Nermo must not require a modified Node.js runtime.

The generated project structure should be compatible with ordinary Node.js module resolution.

## 20.2 JavaScript Module Systems

The initial implementation must support packages using:

* CommonJS.
* ES modules.
* Conditional package exports.
* Standard package entry points.

Nermo should not rewrite package contents to change module behavior.

## 20.3 Build Tools

Nermo should work with popular JavaScript build tools, including:

* Vite.
* Next.js.
* Webpack.
* Rollup.
* esbuild.
* TypeScript.
* Babel.

Compatibility must be validated through integration tests.

## 20.4 Native Packages

Packages that include native binaries or platform-specific optional dependencies require additional handling.

Nermo must consider:

* Operating system.
* CPU architecture.
* Node.js ABI compatibility.
* Optional dependency declarations.
* Platform-specific package selection.

The MVP may initially limit support for native packages.

---

# 21. Error Handling and Diagnostics

Nermo should provide clear, actionable error messages.

Errors should include:

* A short description.
* The affected package or project.
* The relevant filesystem path when appropriate.
* The likely cause.
* A suggested recovery action.

Example:

```text
Error: Package installation failed.

Package: example-package@1.2.0
Reason: Integrity verification failed.

The downloaded package does not match the expected
integrity hash.

The package was not added to the global store.

Try:
nermo install --force
```

The exact recovery command should depend on the cause. Nermo must not suggest bypassing integrity verification as a routine fix.

Diagnostic output should support a verbose mode.

---

# 22. Observability

Nermo should provide structured internal logging.

Recommended logging levels:

* Error.
* Warn.
* Info.
* Debug.
* Trace.

The default output should be concise.

Verbose mode should expose installation phases and package-level operations.

Example:

```bash
nermo install --verbose
```

The tool should also support machine-readable output for scripts.

Example:

```bash
nermo install --json
```

The JSON output schema should be versioned if exposed as a stable interface.

Nermo should not transmit telemetry by default.

---

# 23. Technology Stack

## 23.1 Programming Language

**Rust**

Rust is selected because of its:

* Memory safety.
* Performance.
* Strong concurrency primitives.
* Efficient filesystem operations.
* Cross-platform support.
* Suitable ecosystem for CLI applications.

## 23.2 Recommended Libraries

| Library    | Purpose                           |
| ---------- | --------------------------------- |
| Tokio      | Asynchronous runtime              |
| Reqwest    | HTTP client                       |
| Serde      | Serialization and deserialization |
| Serde JSON | JSON manifests and lockfiles      |
| TOML       | Configuration                     |
| Semver     | Version parsing and comparison    |
| Clap       | CLI argument parsing              |
| Tar        | Archive extraction                |
| Flate2     | Gzip decompression                |
| SHA-2      | Content hashing                   |
| Tracing    | Logging                           |
| Fs2        | Filesystem locking                |
| Tempfile   | Temporary files and directories   |
| Walkdir    | Directory traversal               |
| Thiserror  | Error definitions                 |

Dependencies should be selected based on actual implementation needs.

## 23.3 Project Structure

```text
nermo/
├── Cargo.toml
├── Cargo.lock
├── README.md
├── LICENSE
├── CONTRIBUTING.md
├── CHANGELOG.md
├── .github/
│   └── workflows/
│       ├── ci.yml
│       └── release.yml
├── crates/
│   ├── nermo-cli/
│   ├── nermo-core/
│   ├── nermo-registry/
│   ├── nermo-resolver/
│   ├── nermo-store/
│   ├── nermo-linker/
│   └── nermo-lockfile/
├── tests/
│   ├── integration/
│   ├── fixtures/
│   └── benchmarks/
└── docs/
    ├── architecture.md
    ├── lockfile.md
    ├── configuration.md
    └── compatibility.md
```

A Cargo workspace should be used if the codebase becomes large enough to justify independent crates.

For the first prototype, a single crate with clearly separated modules may be more practical.

---

# 24. MVP Scope

The first release should focus on the smallest feature set that demonstrates the core architecture.

## 24.1 MVP Features

| Feature                   | Priority | Required   |
| ------------------------- | -------- | ---------- |
| Rust CLI                  | P0       | Yes        |
| Read package.json         | P0       | Yes        |
| npm registry client       | P0       | Yes        |
| Download package tarballs | P0       | Yes        |
| Verify package integrity  | P0       | Yes        |
| Extract packages          | P0       | Yes        |
| Global package store      | P0       | Yes        |
| Basic dependency resolver | P0       | Yes        |
| Transitive dependencies   | P0       | Yes        |
| Project-level symlinks    | P0       | Yes        |
| Nermo lockfile            | P0       | Yes        |
| Warm installations        | P0       | Yes        |
| Package removal           | P1       | Yes        |
| Store inspection          | P1       | Yes        |
| Store pruning             | P1       | Yes        |
| Offline installation      | P1       | Yes        |
| Workspace support         | P2       | No         |
| Windows support           | P1       | Target     |
| Native packages           | P1       | Partial    |
| Lifecycle scripts         | P1       | Controlled |
| Content-addressable store | P2       | Future     |
| Import existing lockfiles | P2       | Future     |

## 24.2 MVP Acceptance Criteria

The MVP is considered functional when it can:

1. Install a simple npm project from `package.json`.
2. Resolve and install transitive dependencies.
3. Store packages outside the project directory.
4. Create working project-level links.
5. Reuse previously installed packages in another project.
6. Preserve multiple versions of the same dependency.
7. Generate and read a lockfile.
8. Perform a frozen installation.
9. Avoid unnecessary package downloads during warm installations.
10. Allow the project to run using ordinary Node.js.
11. Allow the project to be installed using npm independently of Nermo.
12. Recover cleanly from interrupted package downloads.

---

# 25. Development Roadmap

## Phase 1: CLI Foundation

**Objective:** Build a functioning Rust command-line application.

Tasks:

* Initialize the Rust project.
* Configure the CLI.
* Implement project discovery.
* Parse `package.json`.
* Implement configuration loading.
* Add structured logging.
* Define shared error types.
* Establish integration tests.

Deliverable:

A CLI that can inspect a JavaScript project and display its dependencies.

## Phase 2: Registry Client

**Objective:** Download and inspect npm packages.

Tasks:

* Implement registry metadata retrieval.
* Implement version metadata retrieval.
* Download package tarballs.
* Support redirects.
* Implement integrity verification.
* Extract package archives safely.
* Handle download failures.

Deliverable:

A functional registry client capable of downloading and extracting a specified package version.

## Phase 3: Global Store

**Objective:** Implement centralized package storage.

Tasks:

* Establish platform-specific store paths.
* Implement package identity.
* Implement package lookup.
* Implement temporary extraction.
* Implement atomic package insertion.
* Add concurrency protection.
* Add store metadata.
* Add package integrity checks.

Deliverable:

A working global package store capable of storing and retrieving packages.

## Phase 4: Dependency Resolver

**Objective:** Resolve ordinary npm dependency trees.

Tasks:

* Implement semantic version range matching.
* Retrieve registry metadata.
* Resolve direct dependencies.
* Resolve transitive dependencies.
* Handle multiple versions.
* Detect circular dependency relationships.
* Define deterministic graph serialization.
* Add resolver tests.

Deliverable:

A resolver capable of generating a complete dependency graph for supported packages.

## Phase 5: Project Linker

**Objective:** Connect globally stored packages to project directories.

Tasks:

* Implement symbolic links.
* Support scoped packages.
* Implement nested dependencies.
* Handle version conflicts.
* Detect existing unmanaged directories.
* Implement safe link replacement.
* Add recovery mechanisms.

Deliverable:

Projects that can use globally stored packages through ordinary `node_modules` directories.

## Phase 6: Lockfile and Installation Engine

**Objective:** Deliver the complete installation workflow.

Tasks:

* Define the lockfile schema.
* Implement lockfile parsing.
* Implement deterministic lockfile generation.
* Implement frozen installations.
* Implement incremental installations.
* Implement rollback where feasible.
* Add end-to-end tests.

Deliverable:

A working `nermo install` command.

## Phase 7: Store Management

**Objective:** Provide package-store maintenance.

Tasks:

* Implement store statistics.
* Track project references.
* Implement unused package detection.
* Implement safe pruning.
* Implement cleanup.
* Add dry-run support.

Deliverable:

A global store that users can inspect and maintain.

## Phase 8: Performance Optimization

**Objective:** Optimize installation performance.

Tasks:

* Benchmark against npm, pnpm and Bun.
* Profile dependency resolution.
* Optimize registry metadata caching.
* Improve concurrent downloads.
* Optimize package lookup.
* Reduce redundant filesystem operations.
* Optimize incremental linking.
* Measure memory usage.

Deliverable:

A documented performance report and a set of validated optimizations.

## Phase 9: Compatibility and Release

**Objective:** Prepare the first public release.

Tasks:

* Test on macOS.
* Test on Linux.
* Test on Windows.
* Validate popular frontend frameworks.
* Test native dependencies.
* Audit archive extraction.
* Review lifecycle script behavior.
* Write documentation.
* Publish binaries.

Deliverable:

Nermo 1.0, subject to successful compatibility and reliability testing.

---

# 26. Testing Strategy

## 26.1 Unit Tests

Unit tests should cover:

* Semantic version parsing.
* Version range matching.
* Manifest parsing.
* Lockfile serialization.
* Package identity.
* Integrity verification.
* Registry metadata parsing.
* Dependency graph construction.
* Link path calculation.

## 26.2 Integration Tests

Integration tests should use isolated temporary directories and test registries or controlled fixtures.

Required scenarios:

* Installing a package.
* Installing a project with multiple dependencies.
* Installing transitive dependencies.
* Installing conflicting dependency versions.
* Reusing packages across projects.
* Running a frozen installation.
* Installing offline.
* Removing dependencies.
* Recovering from interrupted installations.
* Handling corrupt package archives.

## 26.3 Compatibility Tests

Create representative fixture projects using:

* React.
* Next.js.
* Vite.
* TypeScript.
* Express.
* Packages with scoped names.
* Packages with peer dependencies.
* Packages with optional dependencies.

Test actual execution through Node.js rather than merely checking that links exist.

## 26.4 Regression Tests

Every reported installation failure should have a reproducible regression test where practical.

The CI pipeline should run tests on all supported operating systems.

---

# 27. Product Metrics

Nermo's success should be evaluated using measurable engineering metrics.

## 27.1 Performance

* Cold installation duration.
* Warm installation duration.
* Incremental installation duration.
* Dependency resolution duration.
* Package extraction duration.
* Link creation duration.
* Peak memory consumption.

## 27.2 Storage Efficiency

* Global store size.
* Project-level disk usage.
* Number of duplicate package downloads avoided.
* Number of duplicate package extractions avoided.
* Estimated storage saved across projects.

## 27.3 Reliability

* Successful installation rate.
* Failed installation recovery rate.
* Lockfile consistency.
* Integrity verification success.
* Broken link frequency.
* Cross-platform test coverage.

## 27.4 Adoption

For a public release, optional product metrics could include:

* Downloads.
* GitHub stars.
* Community contributions.
* Reported compatibility issues.
* Supported package coverage.

Telemetry should remain opt-in.

---

# 28. Risks and Mitigations

| Risk                               | Impact | Mitigation                                           |
| ---------------------------------- | ------ | ---------------------------------------------------- |
| Complex npm dependency resolution  | High   | Start with a restricted but well-defined subset      |
| Incorrect peer dependency handling | High   | Add explicit peer dependency resolution and tests    |
| Filesystem linking differences     | High   | Implement platform-specific linker backends          |
| Packages modifying shared files    | High   | Treat the store as immutable                         |
| Broken links after store cleanup   | High   | Track references and implement conservative pruning  |
| Registry outages                   | Medium | Cache metadata and support offline installations     |
| Concurrent installation conflicts  | High   | Use package-specific locks and atomic insertion      |
| Lifecycle script incompatibility   | High   | Implement an explicit script policy                  |
| Native package incompatibility     | High   | Test platform-specific dependency behavior           |
| Slow initial installations         | Medium | Optimize concurrency and archive extraction          |
| Lockfile incompatibility           | High   | Version the lockfile and document migration          |
| Deployment detecting Nermo         | Medium | Keep Nermo metadata separate from standard manifests |
| Corrupt package archives           | High   | Verify integrity and extract safely                  |

---

# 29. Future Features

The following features are outside the initial MVP but should influence architectural decisions.

## 29.1 Content-Addressable Store

Store identical package contents only once, even when they are referenced by different package identities.

## 29.2 Workspace Support

Support npm-compatible monorepos with shared dependency resolution.

## 29.3 Package Store Compression

Explore filesystem compression and deduplication strategies.

Compression should not interfere with package access or significantly increase installation latency.

## 29.4 Remote Package Cache

Allow teams to share package artifacts through an optional remote cache.

This should be an independent feature and must not be required for ordinary local use.

## 29.5 Package Store Verification

Provide a command to verify all packages in the global store.

Example:

```bash
nermo store verify
```

## 29.6 Package Store Relocation

Allow users to move their global store to another drive without reinstalling all dependencies.

## 29.7 Project Profiles

Support multiple local dependency environments for different branches or development configurations.

This feature should be considered only after the core package store is stable.

## 29.8 Editor Integration

Potential future integrations include:

* VS Code.
* JetBrains IDEs.
* Terminal-based development environments.

The initial release should remain a standalone CLI.

---

# 30. Open Technical Decisions

The following decisions should be resolved during implementation.

| Decision                 | Proposed direction                 |
| ------------------------ | ---------------------------------- |
| Initial store layout     | Package name and version           |
| Future store layout      | Content-addressable                |
| Default linking          | Symlinks where supported           |
| Windows linking          | Platform-specific abstraction      |
| Lockfile format          | JSON                               |
| Lockfile filename        | `.nermo-lock`                      |
| Registry                 | npm-compatible                     |
| Package integrity        | Registry-provided integrity hashes |
| Lifecycle scripts        | Disabled by default initially      |
| Package storage          | User-specific global directory     |
| Installation concurrency | Configurable and bounded           |
| Project configuration    | Optional                           |
| Deployment integration   | None required                      |
| Initial project support  | Single-package projects            |
| Future project support   | Workspaces and monorepos           |

These decisions should be documented and revisited as implementation experience accumulates.

---

# 31. Definition of Done

Nermo's first stable release should satisfy the following requirements.

### Functionality

* A developer can install ordinary npm dependencies using Nermo.
* Dependencies are stored outside the project.
* Packages are shared across multiple projects.
* Multiple package versions can coexist.
* Node.js can resolve linked packages correctly.
* Lockfiles provide reproducible installations.
* Store cleanup does not delete packages still in use.

### Performance

* Warm installations avoid unnecessary downloads and extraction.
* Unchanged dependencies are not needlessly relinked.
* Benchmarks are reproducible.
* Performance claims are supported by measurements.

### Compatibility

* Projects continue to use standard `package.json` files.
* Projects can be installed independently using npm.
* Existing JavaScript tooling can resolve installed dependencies.
* Supported operating systems pass the integration test suite.

### Security

* Downloaded package integrity is verified.
* Package extraction prevents path traversal.
* Concurrent installations cannot expose partially extracted packages.
* Cleanup operations are restricted to managed files.
* Lifecycle script behavior is explicit.

### Documentation

* Installation instructions are complete.
* CLI commands are documented.
* Configuration options are documented.
* Lockfile format is documented.
* Compatibility limitations are documented.
* Known issues are published.

---

# 32. Final Product Summary

Nermo is a Rust-based package manager focused on shared local dependency storage, efficient package linking and fast repeated installations.

Its central architectural principle is the separation of package storage from individual projects.

A single global store can serve multiple JavaScript projects, reducing duplicated package contents and avoiding unnecessary downloads and extraction.

Nermo should remain compatible with the existing npm ecosystem and should not require deployment providers to recognize its existence.

The initial release should focus on a reliable dependency resolver, an immutable global package store, a correct linker and a deterministic lockfile.

Performance optimization should concentrate on warm installations and multi-project workflows, where shared storage provides the clearest potential advantage.

Nermo's long-term direction is to become a lightweight local development package manager that makes dependency management faster and more storage-efficient without introducing unnecessary complexity into the deployment process.

**Product principle: One package in the global store, many projects using it, and no mandatory changes to how those projects are deployed.**
