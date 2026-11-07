---
name: rust-crate-architecture
description: Use when deciding Rust workspace crate boundaries and dependency direction, such as adding, splitting, merging, renaming, or removing a workspace crate, defining or enforcing dependency layers, fixing a layering violation or a dependency cycle between workspace crates, or choosing between a new crate and a module for new code or for a crate that grew too large. Not for module file layout or visibility; use rust-code-style. A cyclic package dependency that involves a proc-macro crate belongs to rust-macros.
license: BSD-3-Clause
---

# Rust Crate Architecture

This skill decides which crates exist, which crate may depend on which, and
whether new code needs a crate or a module. The `cargo-workflows` skill owns
manifests, dependency inheritance, profiles, features, lockfile policy,
cross-compilation, and build commands. The `rust-code-style` skill owns module
file layout, the `lib.rs` re-export policy, and visibility levels. Use them
together when they are installed.

## Read the graph from cargo

A workspace changes faster than its documentation. Before you move a crate, read
the current graph from cargo, not from a diagram.

```bash
# Members, manifest paths, versions, features, and targets.
cargo metadata --locked --no-deps --format-version 1

# Every edge between path crates, with its kind. It includes optional and
# target-gated edges, so it is the complete list to compare with the layer table.
cargo metadata --locked --no-deps --format-version 1 \
  | jq -r '.packages[] | .name as $n | .dependencies[] | select(.path != null)
           | "\($n) -> \(.name) [\(.kind // "normal")]"' | sort

# Forward dependencies of one crate: what it is allowed to know.
cargo tree --locked -p <crate> -e normal,build --target all

# Reverse dependencies: who knows the crate. Keep --workspace. Inside a member
# directory, cargo tree -i otherwise searches only that member's own tree.
cargo tree --locked --workspace -i <crate> --target all
```

`cargo tree` resolves for the host target unless you pass `--target all`, so an
Android-only or iOS-only edge is invisible in a plain run. It also shows only the
features that the command line enables.

Rules:

- Treat a hand-written architecture document as stale until `cargo metadata`
  agrees with it. Fix the document in the same change that moves a crate.
  Derive crate counts from `cargo metadata`; do not hardcode them.
- Run every command from the directory that holds the workspace `Cargo.toml`,
  or pass `--manifest-path <workspace-root>/Cargo.toml`.

## The layer model

Give every crate exactly one layer. The layer says what the crate is allowed to
know. Names of layers matter less than the direction between them.

| Layer | Contents | May depend on | Must not depend on |
|-------|----------|---------------|--------------------|
| 0 Foundation | Pure data types, error types, format and protocol primitives. No I/O, no global state. | Third-party leaf crates, other Layer 0 crates | Layers 1-3 |
| 1 Domain | Algorithms, state machines, policy, decisions. Deterministic and testable without I/O. | Layer 0, other Layer 1 crates | Layers 2 and 3 |
| 2 Runtime | I/O, sockets, files, timers, task scheduling, process state. | Layers 0-1, other Layer 2 crates | Layer 3 |
| 3 Adapter | FFI boundary crates (`cdylib` / `staticlib`), platform bindings, CLI binaries. | Layers 0-2 | Other adapters. Nothing in the workspace depends on an adapter |

Two derived rules:

- **Place a new crate at the lowest layer that satisfies its dependencies.**
  If the crate compiles without a Layer 2 dependency, it is not a Layer 2 crate.
- **Dependencies flow toward smaller, shared crates and away from adapter
  crates.** An adapter crate is a sink. If something needs to depend on an
  adapter, the thing you need is in the wrong crate.

## Verification

While you iterate, check the changed crate:

```bash
cargo tree --locked -p <changed-crate> -e normal,build --target all   # no higher-layer crate
cargo clippy --locked -p <changed-crate> --all-targets -- -D warnings
```

Once before commit, run the workspace gate:

```bash
cargo metadata --locked --no-deps --format-version 1     # inventory changed as intended
cargo tree --locked --workspace -i <foundation-crate> --target all   # who now depends on the shared crate
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo deny --config deny.toml --locked check
```

Then compare the edge list from [Read the graph from cargo](#read-the-graph-from-cargo)
with the layer table. Every `[normal]` and `[build]` edge must point to a lower
layer, or to the same layer below Layer 3. No such edge may point to an adapter.
A `[dev]` edge may point up. The `rust-security` skill owns the
`cargo-deny` policy.

A green gate does not prove the layering. Clippy and a default cargo-deny policy
accept every acyclic upward edge. Only the edge-list comparison checks
direction.

Checklist:

- [ ] The new or moved crate sits at the lowest layer that compiles.
- [ ] No `[normal]` or `[build]` edge points up, and no cross-stack edge appeared.
- [ ] Test-support crates are under `[dev-dependencies]` only.
- [ ] The manifest inherits the keys the workspace defines, and
      `[lints] workspace = true` when the workspace defines `[workspace.lints]`.
- [ ] The crate has a `[workspace.dependencies]` entry.
- [ ] `#![forbid(unsafe_code)]` is present, or the `unsafe` need is stated.
- [ ] `Cargo.lock` is committed with the manifest change that caused it, and its
      diff holds only that change.
- [ ] The architecture document matches `cargo metadata` again.

## Triage

| Symptom | Cause | Fix |
|---------|-------|-----|
| "cyclic package dependency: package `<name> v<version> (<path>)` depends on itself" | A normal or build edge closes a cycle | Read the `Cycle:` lines to find the back edge. Extract the shared type into a lower-layer crate; delete the back edge |
| A Layer 1 crate has a Layer 2 crate in its tree | Upward dependency | Move the type down, or invert with a trait ([references/dependency-direction.md](references/dependency-direction.md), Fix A and Fix B) |
| E0116 "cannot define inherent `impl` for a type outside of the crate where the type is defined" or E0117 "only traits defined in the current crate can be implemented for types defined outside of the crate" after a split or a type move | An inherent impl or an impl of a foreign trait stayed in the old crate | Move the impl with the type. An impl of a trait that the old crate defines may stay |
| `runtime-b` depends on `runtime-a` | Cross-stack edge | Put the shared type in Layer 0; keep the stacks independent |
| A runtime crate depends on the diagnostics crate | Observation edge points the wrong way | Define the sink as a trait in a contracts crate; invert the edge |
| A fixture crate appears in `cargo tree -e normal` | Test-support crate under `[dependencies]` | Move it to `[dev-dependencies]` |
| E0308 "expected `T`, found `my_crate::T`" in a unit test, with "multiple different versions of crate" | Dev-dependency cycle through a test-support crate | Move the test under `tests/` |
| "cannot update the lock file ... because --locked was passed" right after `cargo new` | `Cargo.lock` does not list the new member | Repair the lock without a build; the `cargo-workflows` skill, when it is installed, owns the procedure |
| "error inheriting `<name>` from workspace root manifest's `workspace.dependencies.<name>`" | Member added without a `[workspace.dependencies]` entry | Add the entry |
| "failed to select a version for the requirement" on a path dependency | `version` in `[workspace.dependencies]` does not match the member | Align the version, or drop it when the policy allows |
| Hand-written manifest without `[lints] workspace = true` in a workspace that defines `[workspace.lints]` | Manifest not generated by `cargo new` | Add the `[lints]` table |
| Member manifest sets `version = "0.1.0"` in a workspace that versions together | Version not inherited | Use `version.workspace = true` |
| Logic added to an FFI or platform adapter crate | Adapter is no longer thin | Move it one layer down, where it can be tested without the platform |
| An upward edge exists only under `[target.'cfg(...)'.dependencies]` | Target gate hides a layering violation | Restructure; the gate does not make the edge legal |
| A crate that holds one type | Crate created without a crate reason | Make it a module; promote it when a real crate reason appears |

## Direction rules

### 1. No upward dependencies

A crate must not depend on a crate in a higher layer. Cargo rejects a cycle of
normal or build edges: "cyclic package dependency: package `<name> v<version>
(<path>)` depends on itself". Cargo accepts an upward edge that closes no cycle.
That edge is still an architecture defect: it drags I/O, platform code, and
heavy dependency trees into crates that were supposed to stay pure. Cargo also
accepts a cycle that closes through a dev-dependency. Rule 3 allows that cycle
for a test-support crate.

Violation:

```toml
# crates/core-config/Cargo.toml   (Layer 0)
[dependencies]
runtime-engine = { workspace = true }   # WRONG: Layer 0 -> Layer 2
```

Fix: find the type that `core-config` actually needs, and move that type down
into Layer 0. The runtime crate then depends on the config crate, not the
reverse.

### 2. Independent stacks stay independent

When a workspace hosts two product areas, model them as two dependency trees
that meet only at shared Layer 0 or Layer 1 crates that neither stack owns.

```text
Stack A:  ffi-a -> runtime-a -> domain-a -> core-config   -> core-types
Stack B:  ffi-b -> runtime-b -> domain-b -> device-config -> core-types
```

An arrow points from a crate to its dependency. The stacks meet only at the
shared Layer 0 crate `core-types`.

`runtime-b` must not depend on `runtime-a`. If both stacks need the same type,
the type belongs in a Layer 0 crate that both already depend on. A cross-stack
edge means one stack can no longer be built, tested, or shipped alone.

### 3. Test-support crates are dev-dependencies only

Golden-file, network-fixture, soak, and platform-logging harness crates go under
`[dev-dependencies]` only. They pull in servers, temporary files, and assertion
machinery that must never reach a shipped artifact.

One exception: a platform-support crate that also carries real runtime helpers
may be a normal dependency, but only of adapter crates, never of a Layer 0 or
Layer 1 crate.

A test-support crate often depends on the crate it tests. That dev-dependency
cycle compiles, but a unit test in `src/` then sees two copies of the crate under
test. The build fails with E0308 "expected `Token`, found `my_crate::Token`" and
the note "there are multiple different versions of crate `my_crate` in the
dependency graph". Put a test that uses such a crate under `tests/`. An
integration test links the one library copy that the support crate also uses.

### 4. Observation is an adapter edge, not a core dependency

A diagnostics, monitoring, or metrics crate may depend on domain and runtime
crates to observe them. Domain and runtime crates must not depend back on it.

```text
ALLOWED:   diagnostics-monitor -> runtime-engine -> domain
FORBIDDEN: runtime-engine -> diagnostics-monitor
```

When a runtime crate must emit an observation, define the sink as a trait or a
handle in a Layer 0 contracts crate. The runtime crate depends on the contract.
The monitor crate implements it. Split a large diagnostics area into a
contracts crate, a runner crate, and one crate per protocol or subsystem, so a
consumer takes only the part it needs. The `rust-observability` skill covers
the sink itself.

### 5. Platform isolation uses `cfg`, not layer breaks

Platform-specific code lives behind `#[cfg(target_os = "...")]`. A target-gated
dependency is still a dependency, and it must respect the same direction rules.

```toml
# Acceptable: a platform binding crate pulled in only on that target.
[target.'cfg(target_os = "android")'.dependencies]
jni = { workspace = true }
```

```toml
# WRONG: a target gate used to smuggle an upward dependency into a domain crate.
[target.'cfg(target_os = "android")'.dependencies]
ffi-adapter = { workspace = true }
```

A conditional dependency that is legal on one target and illegal on another is
a layering violation on every target. A host-only `cargo tree` does not show it;
the edge list in [Read the graph from cargo](#read-the-graph-from-cargo) does.

For a multi-branch platform choice inside a crate, use the standard
`cfg_select!` macro (MSRV 1.95) instead of adding the `cfg-if` crate.

### 6. Adapter crates stay thin

An FFI or platform crate does four things: marshal arguments, manage handle
lifetimes, translate errors, and contain panics. Decisions and algorithms belong
one layer down, where they can be tested without the platform. The `rust-jni`,
`uniffi-boundary`, and `ffi-error-progress-cancel` skills cover the boundary.

## Crate or module?

A new crate is not free. It costs a manifest, a `[workspace.dependencies]`
entry, a `Cargo.lock` entry, a public API for every item a dependent uses (the
`rust-discipline` skill covers that API), and one more unit that every
contributor has to locate. Default to a module inside an existing crate.

Create a crate when at least one of these is true:

- Two otherwise-unrelated crates both need the code, and neither should inherit
  the other's dependency tree.
- The code needs a different unsafe policy, for example the surrounding crate is
  `#![forbid(unsafe_code)]` and this code cannot be.
- The code needs a different dependency set, feature set, or target set: a
  host-only tool, a platform binding, a heavy optional backend.
- The code is a deliberate rebuild boundary: it is stable and large, and the
  code around it changes every day.
- The code is published separately.

A procedural macro always needs its own `proc-macro` crate; the `rust-macros`
skill covers it.

These are not reasons: the file is long, the area "feels like a component",
one type wants a home of its own. Split a crate that grew too large into modules
along its existing responsibilities first. Promote a module to a crate when a
rule above becomes true.

## Creating a new crate

1. **Choose the layer.** Match the crate's contents to the layer table, and list
   the crates it must depend on. Its layer must be at or above the highest layer
   among them. If that puts it above a crate that will use it, the design is
   wrong: stop and re-read [Direction rules](#direction-rules).

2. **Create the package** from the workspace root. `crates/` is the common
   layout; use the directory the workspace already uses for members.

   ```bash
   cargo new --lib crates/<crate-name>
   ```

   Use `cargo init --lib <dir>` for a directory that already exists. Either
   command adds the member to `[workspace] members` unless a glob already
   matches it. It keeps a sorted list sorted and appends to an unsorted one, so
   check the result.

3. **Read the generated manifest.** Cargo writes `<key>.workspace = true` only
   for the keys that `[workspace.package]` defines, and `[lints] workspace =
   true` only when `[workspace.lints]` exists. Add a missing inheritance line by
   hand. The `rust-lints` skill owns the lint set that the `[lints]` table
   inherits.

4. **Add the `[workspace.dependencies]` entry by hand.** Cargo never writes it.
   Without it, a dependent that writes `<crate-name> = { workspace = true }`
   fails with "error inheriting `<crate-name>` from workspace root manifest's
   `workspace.dependencies.<crate-name>`".

   ```toml
   [workspace.dependencies]
   <crate-name> = { path = "crates/<crate-name>" }
   ```

   Add a matching `version` field when you publish the crate, or when the
   workspace denies wildcard dependency requirements even with
   `publish = false`. Read the local policy before you choose the path-only
   form. A `version` that does not match the member version makes every cargo
   command fail with `failed to select a version for the requirement`.

5. **Add dependencies through `workspace = true`.** This is the default because
   it keeps each version requirement in one place.

   ```toml
   [dependencies]
   core-types = { workspace = true }

   [dev-dependencies]
   golden-test-support = { workspace = true }
   ```

6. **Add the safety attribute** to `src/lib.rs`:

   ```rust
   #![forbid(unsafe_code)]
   ```

   Omit it only when the crate has hand-written `unsafe`, including an
   `#[unsafe(no_mangle)]` export: a C ABI or JNI export crate, a driver, a crate
   over a raw C API. A crate that calls C only through a safe wrapper crate
   keeps it. The `rust-unsafe` skill covers the crates that omit it.

7. **Record the member in `Cargo.lock`, then verify.** A new member changes the
   lockfile, so every `--locked` command that resolves the graph (`check`,
   `clippy`, `tree`, `test`) fails with `cannot update the lock file ... because
   --locked was passed` until you update it once. The `cargo-workflows` skill,
   when it is installed, owns the lockfile repair. Expect the lock diff to hold
   the new package and its new dependencies only. Then verify:

   ```bash
   cargo clippy --locked -p <crate-name> --all-targets -- -D warnings
   cargo tree --locked -p <crate-name> -e normal,build --target all
   ```

   Every crate in the tree must sit at or below the new crate's layer.

### Naming conventions

- When a workspace hosts several independent stacks, give each stack one prefix:
  `<stack>-<role>`. The prefix makes a cross-stack dependency visible in a diff.
- Name test-support and utility crates by role, with no stack prefix:
  `golden-test-support`, `network-fixture`.
- Do not encode a layer number in the name. Layers move during a refactor;
  renaming a crate is expensive.
- Cargo maps hyphens in a package name to underscores in the library target, the
  `use` path, and the artifact file name. Renaming a package renames
  `lib<name>.so` or `lib<name>.a` and breaks every loader and binding that names
  it. To rename the package and keep the artifact and the `use` path, set
  `[lib] name = "<old_name>"`. The `[workspace.dependencies]` key, each
  dependent's dependency key, and every `-p <name>` in scripts and CI still
  change.

## Restructuring an existing workspace

Read [references/dependency-direction.md](references/dependency-direction.md)
when you audit an inherited workspace, break an upward edge, or split, merge, or
delete a crate. The short form:

| Situation | Move |
|-----------|------|
| Two crates need the same type | Move the type down to the lowest crate both already depend on |
| A lower crate calls into a higher crate | Invert with a trait defined in the lower crate |
| One crate does two unrelated jobs | Split it, and keep a re-export shim for one release to keep the diff reviewable |
| Two crates always change together and share no other consumer | Merge them, and delete a layer boundary that was never real |

Put a crate move in its own commit, apart from any behavior change. A move plus
a behavior change in one diff is unreviewable.
