# Architectural decision record (ADR) 001: Adopt the Rust build standard for development builds

## Status

Accepted on 8 October 2026. Development, test, lint and typecheck builds use
the parallel `rustc` frontend (`-Zthreads=8`) and, on Linux, the `mold` linker,
while coverage and release builds stay off the fast flags.

## Date

2026-10-08.

## Context and problem statement

Edit-compile cycles are dominated by frontend time and by link time. The
estate's build standard (`rust-build-defaults`) addresses both. Cargo applies
exactly one `rustflags` source, and an assigned `RUSTFLAGS` replaces every
configuration source, so a recipe or workflow step that assigns `RUSTFLAGS`
silently loses the flags unless it restates them. Measurements and shipped
artefacts also need a baseline that the fast flags would disturb.

## Decision drivers

- Faster local and CI development builds without changing what the code means.
- Coverage numbers that do not depend on the fast flags.
- A release that ships from the platform linker.
- A flag lost through a recipe or workflow edit must fail a test, not pass
  quietly.

## Options considered

- Configure the flags only in `.cargo/config.toml`. This leaves every recipe
  and CI step that assigns `RUSTFLAGS` without them.
- Restate the flags in each recipe and step, held by a contract test. This is
  the option taken.
- Run coverage on the development backend. Cranelift does not emit the
  instrumentation coverage needs, so the build would report nothing useful.

## Decision outcome

`.cargo/config.toml` carries the flags in every `rustflags` source, and adds the
`mold` linker flag to the Linux table only. The Makefile composes the same
flags into each development recipe, keeping the caller's own `RUSTFLAGS`.
Coverage assigns its own flags and ignores the caller's. The release recipe
keeps the caller's flags and names neither fast flag, so it ships from the
platform linker; a direct `cargo build --release` takes the configuration's
flags unless `RUSTFLAGS` is assigned. Cranelift is the development-profile code
generator, selected in `.cargo/config.toml`; coverage selects LLVM explicitly
because instrumentation needs it.

`tests/build_standard_contract.rs` reads the Cargo configuration sources and
the commands `make -n` prints for each target, and the `setup-rust` steps of
the workflows, so a flag lost through a recipe or workflow edit fails there.

## Consequences

- Linux builds require `mold`. The flag reaches the linker through `cc`, which
  must be GCC 12.1 or newer, or clang.
- A change to a recipe or workflow step that assigns `RUSTFLAGS` must restate
  the flags, and the contract says which clause fails when it does not.
- Re-measure the suite under Cranelift on the next toolchain bump.

## Addendum, 2026-10-31: Whitaker is isolated from the standard

The decision above says the Makefile composes the standard flags into each
development recipe. That holds for Cargo gate builds, and no longer for
Whitaker. The Concordat `rust-build-defaults` rule (BD-008) reads a direct
installer-managed Whitaker route as its own boundary: Whitaker runs under the
toolchain its installer manages, not this repository's pin, so it takes none of
the development standard.

- `make lint` runs Whitaker as `RUSTFLAGS="-D warnings" $(WHITAKER)` after a
  `cd` to the checkout's parent, with `--manifest-path` naming the checkout.
  The literal `RUSTFLAGS` displaces the `rustflags` sources, so Whitaker gets
  no parallel frontend flag and no `mold` linker flag, and it inherits nothing
  from the caller. Starting in the checkout's parent, Cargo never discovers
  `.cargo/config.toml`, so the Cranelift profile stays out of the
  installer-managed toolchain too.
- Consequence: Whitaker links with the platform linker, and a caller's
  `RUSTFLAGS` no longer reaches it.
- `tests/build_standard_support/make.rs` reads an isolated Whitaker command as
  exempt from the restatement clauses.
- The `unix-only-build-contract` job passes `rustflags: ''` to `setup-rust`, so
  its `cargo check` reads the repository's Cargo configuration instead of the
  action's exported `-D warnings` (BD-009).
- The development profile also sets `incremental = false` and `debug = 0`, and
  `.cargo/config.toml` the native `[unstable] embed-metadata = false` key for
  the dated nightly (the `rust-dev-build-footprint` rule), described in the
  developers' guide.
