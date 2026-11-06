---
name: rust-code-style
description: Use when laying out Rust modules and source files, choosing visibility (pub, pub(crate), pub(super)), writing lib.rs re-exports or a prelude, ordering items, fixing import grouping, choosing thiserror or anyhow, writing rustdoc Errors and Panics sections, or reviewing a diff for layout and readability. Triggers on mod.rs, re-export, unreachable_pub, Iterator for_each, or flat_map over a Result.
license: BSD-3-Clause
---

# Rust Code Style

This skill holds the layout and readability decisions that `rustfmt` cannot make. `rustfmt`
owns line width, brace placement, wrapping, and the sort order inside each contiguous block of
`mod` declarations or `use` imports. The `rust-lints` skill owns `clippy.toml`, `rustfmt.toml`,
and the workspace lint tables. The `rust-crate-architecture` skill owns crate boundaries and
dependency direction.

Most rules here are catalog defaults, not Rust facts. Each one carries its reason, so a team can
override it on purpose. The `flat_map` and `lines()` rules are exceptions: they prevent lost
errors and hangs, so they hold everywhere.

## Checks

Use the tool where one exists. Review the rest by hand with the [checklist](#review-checklist).

| Rule | Check |
| --- | --- |
| Formatting, and the order inside each `mod` or `use` block | `cargo fmt --all --check` |
| No `pub` item that the crate root cannot reach | rustc `unreachable_pub` (allow by default; set it to `warn` in the workspace lint table) |
| An inline `#[cfg(test)] mod tests` is the last item | `clippy::items_after_test_module` (warn by default; needs the test cfg, as in `cargo clippy --all-targets`) |
| `# Errors` and `# Panics` doc sections | `clippy::missing_errors_doc`, `clippy::missing_panics_doc` (pedantic) |
| `# Safety` doc section | `clippy::missing_safety_doc` (warn by default) |
| Broken intra-doc links, public docs that link private items | The `cargo doc` gate in the `rust-lints` skill. It passes `--document-private-items`, so it also checks the links in private docs |
| Doc examples compile and pass | `cargo test --locked --workspace --doc` (library targets only) |
| No `Iterator::for_each` | `disallowed-methods` in `clippy.toml` |
| No `filter_map(Result::ok)` or `flatten()` on `io::Lines` | `clippy::lines_filter_map_ok` (warn by default) |

No tool checks import groups, the rest of the item order, caller-before-callee order, the depth
exception for `mod.rs`, or the early-return rule. A green run proves nothing about them.

## Module layout

Use `name.rs` next to a `name/` directory for every module that has children. This is the
default because the module root then sits next to its children, and every editor tab has a
distinct name. Use `mod.rs` only three levels deep or more. Do not mix the two patterns at one
directory level: a reader then cannot predict where a module root is.

`lib.rs` declares the modules first, then re-exports the public API item by item, with no glob.
A glob re-export makes every new `pub` item public API with no review. A `pub mod prelude` is
the one module that a caller may glob-import. Keep it short, and build it from explicit
`pub use` items. Avoid `#[path]`.

Read [references/module-layout.md](references/module-layout.md) when you create `lib.rs` or a
prelude, when a downstream crate reports E0659, E0034, or "`X` is ambiguous" after a prelude
change, when you enforce the layout with a lint, or when you include `build.rs` output.

## Visibility

| Level | Use it for |
| --- | --- |
| private (default) | An implementation detail of one module |
| `pub(super)` | A helper that only the parent module calls |
| `pub(crate)` | An item that other modules of the same crate use |
| `pub` | The public API, re-exported from `lib.rs` |

Start private. Widen one step only when a concrete caller in another module needs the item.
Remove the widening when the caller goes away.

`pub(super)` follows the module tree, so check its callers again when you move the module. Do
not write `pub(in path)` without a written reason: it names a module path, and a module move
breaks the build.

`unreachable_pub` flags a `pub` item that no path from the crate root reaches, and suggests
`pub(crate)`. Fix the item; do not suppress the lint.

Do not widen visibility for a test. A child `mod tests` in the same file already reads the
private items of its parent. If a test needs an item from another module, the item is part of
the `pub(crate)` API, or the test belongs in the module that owns the item.

## File structure order

Inside a `.rs` file, place the items in this order:

1. Crate and module attributes (`#![forbid(unsafe_code)]`,
   `#![doc = include_str!("../README.md")]`).
2. `mod` declarations: private first, then `pub mod`, with a blank line between the two groups.
   The `#[cfg(test)] mod tests` declaration goes at the bottom (step 10).
3. `use` imports (see [Import style](#import-style)).
4. `pub use` re-exports.
5. Constants and statics.
6. Type definitions: structs, enums, type aliases.
7. Trait definitions.
8. `impl` blocks: inherent first, then trait impls.
9. Free functions: public first, then private helpers.
10. `#[cfg(test)] mod tests`.

Put a blank line between steps 3 and 4 too. rustfmt (`reorder_modules` and `reorder_imports`,
stable and on by default) sorts each contiguous block of `mod` or `use` lines by name and
ignores visibility. Without the blank lines, `cargo fmt` mixes `mod` with `pub mod`, and moves
`pub use crate::...` above `use std::...` (rustfmt 1.9.0, Rust 1.98.1).

Steps 2 to 4 set their own order. Inside each of steps 5 to 9, put public items before private
items. The file then reads as a table of contents: the API surface comes before the internals.
Put a type before its `impl` blocks, because the reader needs the data shape before the
methods. Put inherent impls before trait impls, because the inherent methods are the core API.

## Import style

Separate the imports into four groups, with one blank line between the groups:

```rust
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use app_protocol::FrameKind;
use app_telemetry::SpanId;

use crate::types::SharedState;
use crate::util::format_duration;
```

The order is: `std` (and `core`, `alloc`), then external crates, then workspace crates, then
`crate::`, `self::`, and `super::`.

Stable `rustfmt` sorts the imports inside a group. It does not move an import between groups,
because `group_imports` is unstable. Keep the groups by hand, and keep the `pub use` block
apart from them with a blank line (see [File structure order](#file-structure-order)).

The sort order inside a group depends on the style edition. Style edition 2024 uses version
sorting and puts `u8` before `u16`; style editions 2015 to 2021 put them the other way round
(rustfmt 1.9.0, Rust 1.98.1). A direct `rustfmt` call, for example from a pre-commit hook, can
therefore disagree with `cargo fmt`, and `cargo fmt --check` then fails in CI. The `rust-lints`
skill owns the `edition` and `style_edition` keys in `rustfmt.toml`. The `cargo-workflows` skill
says when to change them during an edition migration.

Use one braced import for two or more items of one module: `use std::sync::{Arc, Mutex};`.
Stable `rustfmt` does not merge imports, because `imports_granularity` is unstable.

Import the items and traits that the file uses often. Write the full path at the call site for
an item that the file uses once, such as `std::time::Instant::now()`. This default keeps the
import block short and reduces merge conflicts.

## Function rules

### Caller before callee

Place a calling function before the functions it calls. The reader then follows the code from
top to bottom.

### Iterator chains and collectors

Keep every closure in a `.map()`, `.filter()`, or `.collect()` chain pure. Write a side effect
as a `for` loop.

Ban `Iterator::for_each` through `disallowed-methods` in `clippy.toml`. This is a catalog
default, not a Rust rule: a `for` loop covers every side-effecting use and shows the effect in
plain sight. `clippy::needless_for_each` (pedantic) is a lighter built-in check, but it does not
fire on a chain such as `.filter(..).for_each(..)`. The `rust-lints` skill holds the
`clippy.toml` entry.

Never `flat_map` or `flatten` over a `Result`. `Err` yields zero items, so the failure
disappears, and no lint or type error reports it. Use `collect` into a `Result` to stop at the
first error, or `partition` to keep both halves. Use `filter_map` with `.ok()` only when the drop
is the intent.

Never skip the errors of `BufRead::lines()` with `filter_map`, `flat_map`, or `flatten`: the
read can hang. A reader that fails on every call, such as a directory opened as a file on Unix,
makes `lines()` yield `Err` forever, so the first `next()` never returns. Collect into
`io::Result<Vec<String>>` to propagate the error. Use `map_while(Result::ok)` only when a silent
stop at the first error is the intent.

Read [references/iterator-style.md](references/iterator-style.md) when you judge whether an
expression or closure hides a side effect, choose a collector for fallible items, or see a
benchmark that argues for `for_each`.

### Business paths use `if`/`else` and `match`

Keep a guard return (`return`, `let ... else`) for bookkeeping: a null check, a handle check, a
permission guard. This rule does not limit `?`, which propagates an error. Write mutually
exclusive business paths as `if`/`else` or `match`. This is a catalog default: one `match`
shows every alternative in one place, while a chain of early returns makes the reader hold
every earlier condition to know when the last line runs.

## Error handling

| Context | Default | Pattern |
| --- | --- | --- |
| Library error type | `thiserror` | `#[derive(Debug, thiserror::Error)]` on an enum |
| Binary or CLI | `anyhow` | `anyhow::Result`, `.context()`, `.with_context()` |
| Test | `.expect("<what should have happened>")` | On the value under test. The `rust-tdd` skill owns test design |
| Propagation | `?` | `.unwrap()` in tests and examples only |

A library returns a typed error, so the caller can match on the variant:

```rust
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("the request timed out after {0:?}")]
    Timeout(std::time::Duration),

    #[error("the handle is not valid")]
    InvalidHandle,

    #[error("transport failure")]
    Transport(#[from] std::io::Error),
}
```

Do not put `anyhow::Error` in the public signature of a library crate. It erases the variants,
so the caller can only print the message.

A binary reports the error to a person. Add context at each layer:

```rust
use anyhow::Context as _;

fn load(path: &std::path::Path) -> anyhow::Result<String> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read the config file {}", path.display()))?;
    anyhow::ensure!(!text.trim().is_empty(), "the config file {} is empty", path.display());
    Ok(text)
}
```

Propagate with `?` and let the caller decide. In non-test code, `.expect("<invariant>")` is
acceptable only for a documented, unconditional invariant. The `rust-discipline` skill owns that
rule and the lock-poisoning policy. The `rust-panic-safety` skill owns panic policy at a
boundary. The `ffi-error-progress-cancel` skill owns error mapping across an FFI boundary.

## Rustdoc contract

A doc comment on a public item is part of the API. Three sections carry information that the
signature cannot. `cargo doc` renders each one as a heading, so a reader finds it in the same
place every time.

| Section | Required on | States |
| --- | --- | --- |
| `# Errors` | Every public `fn` that returns `Result` | Which variants occur, and what causes each one |
| `# Panics` | Every public `fn` that can panic | The exact condition. Write "Never" when a reader would expect a panic |
| `# Safety` | Every public `unsafe fn` | What the caller must guarantee. See the `rust-unsafe` skill |

```rust
use std::path::Path;

pub enum ConfigError { NotFound, PermissionDenied, InvalidUtf8 }

/// Reads a configuration file and parses it as UTF-8.
///
/// # Errors
///
/// - [`ConfigError::NotFound`] if `path` does not exist.
/// - [`ConfigError::PermissionDenied`] if the process cannot read it.
/// - [`ConfigError::InvalidUtf8`] if the bytes are not valid UTF-8.
///
/// # Panics
///
/// Never. Every failure is reported through the returned `Result`.
pub fn read_config(path: &Path) -> Result<String, ConfigError> {
    todo!()
}
```

Turn on `missing_errors_doc` and `missing_panics_doc` for a published crate.
`missing_safety_doc` is on by default.

### Link with intra-doc links, not URLs

Write ``[`ConfigError::NotFound`]`` and let rustdoc resolve it. The link then follows a rename,
and `cargo doc` reports it when the target disappears. A hand-written URL to docs.rs pins a
version and breaks silently. The `cargo doc` gate in [Checks](#checks) fails on an unresolved
link and on public docs that link a private item.

### Doc examples are tests

`cargo test` compiles and runs every doc example in a library target. A binary target runs no
doc examples. Use `?` in an example by returning a `Result`, and hide the scaffolding with a
leading `#`:

```text
/// ```
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let config = mycrate::read_config("app.toml".as_ref())?;
/// assert!(!config.is_empty());
/// # Ok(())
/// # }
/// ```
```

Rustdoc compiles a line that starts with `# ` but does not show it. Use `no_run` for an example
that must compile but must not run. Use `ignore` only when the example cannot compile at all.
`ignore` skips compilation, so nothing reports the example when it goes stale.

## Naming

- The rustc lints `non_snake_case`, `non_camel_case_types`, and `non_upper_case_globals` enforce
  the case rules. Do not suppress them, except on names that a foreign ABI fixes (bindgen
  output, a hand-typed `Java_*` export on jni 0.21). Suppress those on the smallest item, with a
  reason (the `rust-lints` skill). On jni 0.22, `#[jni_mangle]` generates the `Java_*` symbol
  from a snake_case function (the `rust-jni` skill).
- Name a test function after the behavior it proves: `fn decoder_rejects_truncated_frame()`.
  The failure output then reads as a sentence.
- Do not abbreviate a public API name. An abbreviation in a local binding is fine when the
  context is clear.
- Write a crate name with hyphens in `Cargo.toml` (`app-protocol`). The path in code then uses
  underscores (`app_protocol`).
- Give the crates of one workspace one shared prefix, so an import shows at a glance whether the
  item comes from the workspace or from a third party.

## Review checklist

Use this list when you review a diff for layout and readability.

- [ ] A new module with children uses `name.rs` plus `name/`, not `mod.rs`.
- [ ] `lib.rs` declares the modules first, then re-exports item by item, with no glob.
- [ ] A prelude uses explicit `pub use` items.
- [ ] Every new item has the narrowest visibility that its callers need. `unreachable_pub` is
      clean.
- [ ] No `pub(in path)` without a written reason.
- [ ] Each file follows the ten-step order, with public items before private ones in steps 5 to
      9, and a blank line after the private `mod` block and before the `pub use` block.
- [ ] The imports are in four groups, separated by blank lines.
- [ ] A caller function comes before the functions it calls.
- [ ] No closure in an iterator chain has a side effect, and no `for_each` appears.
- [ ] No `flat_map` or `flatten` over a `Result`, and no `filter_map`, `flat_map`, or `flatten`
      on `lines()`.
- [ ] Business paths use `if`/`else` or `match`, not a chain of early returns.
- [ ] Library errors use `thiserror`; `anyhow` stays out of the library public API.
- [ ] No `.unwrap()` outside tests and examples. Each `.expect()` states a real invariant.
- [ ] A crate with no hand-written `unsafe` carries `#![forbid(unsafe_code)]`.
- [ ] Each public `Result`, panicking, or `unsafe` function has its rustdoc section.

## Related skills

Use these skills when they are installed.

| Skill | Use it for |
| --- | --- |
| `rust-lints` | `clippy.toml`, `rustfmt.toml`, workspace lint tables, `disallowed-methods` |
| `rust-crate-architecture` | Crate splits, dependency direction, and the crate public API surface |
| `rust-discipline` | Signature review, `.expect` policy, and API anti-patterns |
| `rust-panic-safety` | Panic policy, unwind safety, and `catch_unwind` |
| `rust-unsafe` | Crates that cannot use `#![forbid(unsafe_code)]`, and `# Safety` contracts |
| `ffi-error-progress-cancel` | Error translation across an FFI boundary |
| `rust-tdd` | Test placement and the test-first loop |
