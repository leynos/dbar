# Developers' guide

This guide orients contributors working on `dbar`, a CLI that renders a
tmux-ready status segment and installs the tmux configuration snippet that
invokes it. It covers module responsibilities, the dependency-injection seams
used for hermetic testing, the tooling gates enforced in CI, and the test
layers and lint constraints that shape test code.

## Architecture overview

`src/lib.rs` is the crate root. It declares the module tree and exposes three
entry points invoked from `main`: `run_status`, `run_refresh`, and
`run_install`, dispatched from `run()` based on the parsed `DbarCommand`.

- `config/mod.rs` — CLI parsing (`clap`) and configuration merging
  (`ortho_config`). `Cli`/`Commands` define the `status`, `refresh`, and
  `install` subcommands; `StatusArgs`, `RefreshArgs`, and `InstallArgs` are
  `OrthoConfig` structs merged from CLI flags, environment variables (`DBAR_`
  prefix), and configuration files. `load_command()` returns a `DbarCommand`.
  Test fixtures live under `config/tests/`.
- `command/mod.rs` — the `CommandRunner` trait and `CommandSpec` builder used
  to shell out to `git`, `gh`, and `tmux`. See "Dependency-injection seams"
  below.
- `git/mod.rs` — free functions `project_name` and `git_status` that run
  `git` probes (in `git/probes.rs`) through a `&dyn CommandRunner`.
  `project_name` returns a `ProjectNameOutcome`, pairing the resolved
  `ProjectName` with the `GitProbeFailure` the fallback absorbed, if any: the
  heuristics yield the same name whether the origin probe answered or could not
  be run, so without the outcome the two are indistinguishable. A non-zero exit
  is git *answering* — it is how `git remote get-url origin` reports no such
  remote — so only an unrunnable probe is recorded as a degradation.
  `git_status` returns a `GitStatusOutcome` — `Available(GitStatusReport)`,
  `NotARepository`, or `Unavailable(GitProbeFailure)` — so a caller can tell a
  missing repository apart from a failed or unparsable probe; `GitStatusReport`
  carries the `GitStatus` snapshot alongside any field-level `GitProbeFailure`s
  the fallback policy absorbed. See the module's fallback-policy table for what
  each outcome renders. Before running `git status`, the worktree probe
  preflights tracked paths with `git check-attr`; if repository attributes
  select a filter, or the preflight cannot complete, the status probe is
  skipped so repository-controlled filter commands are not executed.
- `github/mod.rs` — the `GitHubClient` trait and its two implementations,
  `GhCliClient` (backed by the `gh` CLI via `CommandRunner`) and
  `MockGitHubClient` (a fixed value, wired up when `--github-mock-pr` is
  supplied).
- `tmux/mod.rs` — `TmuxContext` and `resolve_context`, which fills in missing
  session/window/pane/socket fields by querying `tmux display-message` through a
  `&dyn CommandRunner`. `resolve_context` returns a
  `TmuxResolution { context, outcome }` rather than a bare `TmuxContext`, so a
  caller can tell a pre-resolved context apart from one queried cleanly, one
  with malformed fields, or one where tmux was unavailable. See the module's
  fallback-policy table for what each outcome renders.
- `cache/mod.rs` — resolves the XDG cache directory (via `directories`) and
  performs TTL-checked reads and atomic temp-file-then-rename writes of cached
  PR lookups, plus the bounded retention sweep described under "Cache
  retention" below. `CacheReader` is the read-only port used by status;
  `CacheStorage` adds the sweep and write operations reserved for refresh.
- `status/mod.rs` — `build_status_report` orchestrates a single, cache-only
  status line: it receives the already-resolved project directory, probes git,
  reads a fresh PR value when present, resolves tmux context, renders the clock
  label, and passes everything to `render::render_status_line`. It returns a
  `StatusReport { line, diagnostics }`, where `diagnostics` is a
  `StatusDiagnostics` collecting every typed probe failure the fallback policy
  absorbed. `refresh_pr_cache` is the explicit boundary for live GitHub lookups
  and cache writes; the pure lookup policy lives in `status/pr/mod.rs` (see
  below), and `status/clock.rs` and `status/cache_key.rs` hold the
  clock-rendering and cache-key-hashing helpers respectively.
- `status/pr/mod.rs` — pure PR lookup policy: `decide` turns a completed
  GitHub lookup (plus the cache outcome the caller already read) into a
  `PrDecision` naming the PR number to render and a `PersistRequest`, without
  performing any I/O itself. `status/mod.rs` applies this policy only from
  `refresh_pr_cache`; `build_status_report` reads the cache without invoking
  GitHub or writing it, which keeps every policy branch testable without a
  filesystem.
- `render/mod.rs` — pure rendering: `RenderContext` plus `render_status_line`
  assemble the tmux `#[...]` style tags and Powerline-style glyphs into the
  final string, with optional right-alignment to a client width.
- `types.rs` — domain newtypes (`ProjectName`, `BranchName`, `AheadCount`,
  `BehindCount`, `PrNumber`, `CacheTtlSeconds`, `StatusPosition`) that avoid
  passing bare `String`/integer values between modules.
- `install/mod.rs` — `install` accepts a configuration path, position, run
  mode, and width, then inserts or updates a marker-delimited tmux snippet in
  a configuration file. It backs up the previous contents before replacing the
  target atomically:
  the new contents are written to a uniquely named temporary file, which
  inherits the target's permissions, then renamed over the target. `RunMode`
  (`DryRun`/`Write`) and `Width` (`Full`/`Plain`) replaced what were originally
  two adjacent `bool` parameters: two booleans of the same type can be
  transposed without the compiler noticing, and transposing these two would
  silently turn a preview into a write of the wrong variant, so each is its own
  enum instead. The module is split by responsibility: `snippet.rs` decides
  what the config should contain (marker handling and snippet assembly, pure
  and disk-free), and `fs.rs` is the capability-based filesystem and locking
  layer underneath it, using `cap_std`/`camino` for path-capable, UTF-8-only
  filesystem access and an `flock`-backed sibling lock file to serialize
  concurrent installs against the same config. Tests live in `tests.rs` (unit
  coverage of `install()` and its helpers), `quoting_tests.rs` (snippet
  quoting/escaping), and `property_tests.rs` (`proptest`-driven property
  coverage).
- `error.rs` — `DbarError`, the top-level error enum returned by `run()`,
  wrapping `CacheError`, `config::ConfigError` (itself wrapping
  `ortho_config::OrthoError`), `InstallError`, and `std::io::Error`.

### Unix-only build contract

The crate is deliberately restricted to Unix targets. `command/mod.rs` uses a
compile-time `compile_error!` for every non-Unix build, with the diagnostic
`dbar supports Unix targets only`. This is a contract rather than a tmux
convenience: command timeouts put children in POSIX process groups so the
whole descendant tree can be signalled, and the install transaction uses
`flock` to serialize concurrent updates. Providing stubs on another platform
would make those safety guarantees false.

The `unix-only-build-contract` CI job installs the
`x86_64-pc-windows-gnu` Rust target and runs `cargo check` for it. The job
passes only when that check is rejected and its output contains the documented
compile-time diagnostic, so the platform boundary remains tested.

### End-to-end assembly of a status line

1. `run_status` in `src/lib.rs` constructs a `RealCommandRunner`, a
   `DefaultClock` (from `mockable`), and a `FileCacheStorage`, then resolves the
   project directory at the CLI boundary and passes a `StatusDependencies` value
   containing the runner, clock, and cache's read port to
   `status::build_status_report`. It does not construct a GitHub client: status
   is a read-only query.
2. `status::build_status_report` receives that directory, then calls
   `git::project_name` and `git::git_status`.
3. If PR display is enabled and a git branch was found, status reads the
   branch's cache entry through `cache::load_cached_value`. A fresh value is
   rendered; a missing, expired, or unreadable entry leaves the PR segment
   absent until `dbar refresh` performs the live lookup. Status never calls
   GitHub, sweeps expired entries, or writes the cache.
4. `run_refresh` constructs the GitHub client and a `FileCacheStorage`, then
   injects its `CacheStorage` port into `status::refresh_pr_cache`. That
   boundary applies `status::pr::decide` to the lookup result and writes
   successful values through
   `cache::store_cached_value`; failed lookups are not cached, so a transient
   network error does not poison the PR value for the whole TTL.
5. `tmux::resolve_context` fills in any tmux fields not already supplied on
   the command line by querying `tmux display-message`.
6. `render::render_status_line` combines the project, git, PR, tmux, and
   clock segments into the final tmux-ready string. `build_status_report`
   returns that string as `StatusReport::line` and keeps the absorbed failures
   separately in `StatusReport::diagnostics` (`StatusDiagnostics`).
7. `run_status` writes only `StatusReport::line` to stdout. If
   `DBAR_DIAGNOSTICS` is set, or `--diagnostics` is supplied, it mirrors safe,
   bounded diagnostic categories to stderr. Diagnostics never include raw URLs,
   paths, filenames, or command stderr, and they are never mixed into stdout,
   so the tmux status-line contract is unchanged. See `report_diagnostics` in
   `src/lib.rs`.

### Cache boundary

The cache ports keep filesystem concerns out of status policy:

- `CacheReader` resolves the cache directory and loads one entry with a clock
  and TTL. Its result is `Fresh`, `Expired`, or `Missing`; it never creates,
  updates, or removes a file. `build_status_report` receives this read-only
  port and uses it while rendering status.
- `CacheWriter` owns the mutating operations: the bounded retention `sweep`
  and `store` for a refreshed value. `refresh_pr_cache` receives the combined
  storage port and invokes these operations as required by the cache result
  and persistence policy.
- `CacheStorage` is the `CacheReader + CacheWriter` composition required by
  refresh. It is not needed by status, which must remain a cache-only query.

`FileCacheStorage` is the filesystem adapter implementing all three ports. The
CLI composition functions construct it and pass its `CacheReader` view to
`run_status`, or its `CacheStorage` view to `run_refresh`. The cache module
maps filesystem errors to the adapter-neutral `CacheFailure` categories before
they reach status, and tests can provide narrower port implementations without
creating files or invoking GitHub.

### Cache retention

Each `(project directory, branch)` pair hashes to its own
`pr_<16 hex digits>.json` file, so entries would otherwise accumulate for every
branch a checkout has ever had, including branches long since deleted.
`cache/mod.rs` reclaims them under this policy:

- **Trigger.** A read never sweeps. `load_cached_value` reports an entry past
  its TTL as `CacheLookup::Expired` and leaves it on disk;
  status reads the current entry but never lists the directory. After resolving
  the cache directory, every explicit `dbar refresh` invokes the bounded sweep
  before resolving the current key, regardless of whether that key is fresh,
  missing, or expired. The reclamation is therefore visible at the refresh call
  site rather than hidden behind a `load_*` name. Writes never sweep either
  because `store_cached_value` is given no TTL to judge entries by.
- **Bound.** One sweep lists at most 256 names, opens and parses at most 16
  of them, and removes at most 8 files. A backlog is therefore cleared across
  successive runs rather than in one unbounded pass on the hot path.
- **Ownership.** The cache directory may be shared, so a file is removed only
  when all three of these hold: its name is `pr_` followed by exactly 16
  lowercase hex digits and `.json`; it is a regular file whose contents
  deserialize as a cache entry; and that entry's own recorded timestamp puts it
  past the TTL. Anything merely named like an entry — `pr_deadbeef.json`,
  `pr_0123456789abcdef.json.tmp`, uppercase hex, or an identically named
  directory — is left alone, as is any file whose contents do not parse.

Expiry is judged from the entry's recorded timestamp rather than from file
metadata, so an entry another dbar process has just refreshed reads as fresh. A
file that vanishes between the listing and the removal counts as success:
another process reclaimed it first. Any other removal failure is reported as a
retention-sweep failure alongside the current lookup result. The caller can log
it and carry on with a fresh lookup without discarding a usable status value.

## Dependency-injection seams

`dbar` shells out to `git`, `gh`, and `tmux`, and reads the wall clock. Tests
must exercise the parsing and orchestration logic without invoking real
processes or mutating the environment, so trait boundaries carry external
process and cache interaction, and the `mockable` crate's `Clock` trait carries
time:

- `command::CommandRunner` —
  `fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, CommandError>`.
  `RealCommandRunner` executes real processes (with a timeout, process-group
  termination, and concurrent pipe draining; see "Real command execution"
  below). The trait carries `#[cfg_attr(test, mockall::automock)]`, so test
  builds also get a `MockCommandRunner` that returns canned `CommandOutput`s or
  errors for the `CommandSpec`s a test expects.
- `github::GitHubClient` — its `pr_number` method receives a project directory
  and branch and returns an optional `PrNumber` or `GitHubError`.
  `GhCliClient` wraps a `&dyn CommandRunner` to shell out to `gh`;
  `MockGitHubClient` returns a fixed, pre-configured PR number and is the
  concrete type wired up for `--github-mock-pr`, but any test can implement the
  trait directly for finer control (see `StubGitHubClient` in
  `src/status/tests.rs`, whose `Reply::Failure` variant stands in for a network
  or rate-limit error).

- `cache::CacheReader` and `cache::CacheStorage` — the read-only port used by
  `status` and the read/write port reserved for `refresh`, respectively.
  `FileCacheStorage` is constructed at the CLI composition boundary, so the
  status policy does not depend on the filesystem adapter.

These traits exist so unit and behavioural tests stay hermetic: no test needs
network access, a real `git`/`gh`/`tmux` binary, or environment-variable
mutation to exercise the orchestration logic in `git/mod.rs`, `tmux/mod.rs`, and
`status/mod.rs`.

`mockall` is an approved dependency of this repository, and every seam it
covers must be doubled by injecting a generated mock rather than a bespoke
hand-written stub. A test needing a `CommandRunner` therefore constructs a
`MockCommandRunner`, configures `expect_run()` with a `mockall::predicate`
matcher over the expected `&CommandSpec`, and states the return value and the
call count; hand-rolling a type that implements `CommandRunner` is not
acceptable.

Two expectation styles are in use, and either is fine so long as the test reads
clearly:

- One expectation per spec, keyed by `with(predicate::eq(spec))`, as in
  `src/github/` and `src/tmux/tests.rs`. This makes the specification itself an
  assertion, because a query built from the wrong arguments matches nothing and
  fails the test.
- A single expectation whose closure switches on the spec it is handed, as in
  `src/git/tests.rs`. This suits probes whose canned answers a fixture supplies
  and individual tests then override, because separate expectations are matched
  in declaration order and the fixture's defaults are declared first.

Call counts are expressed with mockall's own counting — `times(1)`,
`times(1..)`, or `never()` — rather than a counter threaded through a stub, so
a violated expectation fails the test where it happens or when the mock is
dropped.

## Tooling and gates

The `Makefile` wraps the commands contributors are expected to run before
committing:

- `make build` / `make release` — build the debug or release binary.
- `make test` — `cargo test --all-targets --all-features` with `-D warnings`.
- `make lint` — runs `cargo doc --no-deps`, then
  `cargo clippy --all-targets --all-features -- -D warnings`, then the Whitaker
  Dylint suite (`whitaker --all`) against the same targets and features, and
  finally the spelling gate (see below).
- `make fmt` — runs `cargo fmt --all` and `mdformat-all`.
- `make check-fmt` — verifies formatting without modifying files
  (`cargo fmt --all -- --check`).
- `make typecheck` — runs `cargo check` for every target with every feature
  enabled.
- `make markdownlint` — runs `markdownlint-cli2` over every Markdown file,
  then the spelling gate.
- `make spelling` — regenerates `typos.toml` from
  `scripts/generate_typos_config.py` and runs the pinned `typos` release
  against every tracked Markdown file, enforcing en-GB-oxendict spelling.
- `make nixie` — validates Mermaid diagrams embedded in Markdown files.

`typos.toml` is generated output; it is regenerated by `make spelling` (and
therefore by `make lint` and `make markdownlint`) from the spelling policy under
`scripts/`. Repository-only exceptions belong in `typos.local.toml`;
hand-editing `typos.toml` is not supported and any edits are overwritten on the
next generation run.

## Testing guidance

Tests are organized in three layers:

1. Unit tests — `#[cfg(test)] mod tests` declarations colocated with the
   module under test, backed by a sibling `tests.rs` (or, for `config/`, a
   `tests/` directory): `src/command/mod.rs`/`src/command/tests.rs`,
   `src/cache/mod.rs`/`src/cache/tests.rs`, `src/git/mod.rs`/`src/git/tests.rs`,
   `src/tmux/mod.rs`/`src/tmux/tests.rs`, `src/render/mod.rs`/
   `src/render/tests.rs`, `src/status/mod.rs`/`src/status/tests.rs`,
   `src/status/pr/mod.rs`/`src/status/pr/tests.rs`, `src/install/mod.rs`/
   `src/install/tests.rs` (plus `src/install/quoting_tests.rs` and
   `src/install/property_tests.rs`, covering snippet quoting and property-based
   coverage respectively), and `src/config/mod.rs`/`src/config/tests/` (split
   into `mod.rs`, `arguments.rs`, and `files.rs`). Cases use `#[rstest]`, with
   `#[case]` parameterization for table-style coverage and `#[fixture]` for
   shared setup such as temporary directories.
2. Behavioural tests — `rstest-bdd` scenarios under `tests/rstest_bdd/`,
   driven by a `.feature` file (`status.feature`) and step implementations in
   `status_steps.rs`, loaded from the `tests/rstest_bdd_tests.rs` crate root.
3. End-to-end snapshot tests — `tests/e2e/`, using `assert_cmd` to invoke
   the built `dbar` binary against a temporary git repository and `insta` to
   snapshot the rendered status line, loaded from `tests/e2e_tests.rs`.

### Lint constraints that shape test code

The workspace-wide Clippy profile (`Cargo.toml`'s `[lints.clippy]`) denies
`unwrap_used` and `expect_used` everywhere, but `clippy.toml` sets
`allow-expect-in-tests = true`, which narrows that allowance to `#[test]` and
`#[rstest]` function bodies specifically. In practice, for files under `src/`:

- `expect(...)` is permitted only inside a `#[test]`/`#[rstest]` function
  body, never inside a `#[fixture]` body or a plain helper function, even one
  only ever called from tests. Fixtures and helpers must return `Result` and
  propagate failures with `?`, or use an explicit `panic!` with a clear message
  when a `Result` return is not possible (for example in an `rstest-bdd`
  `#[fixture]`, whose return type is fixed by the framework; see `world()` in
  `tests/rstest_bdd/status_steps.rs`).
- Every `#[cfg(test)] mod tests` block needs a `//!` inner doc comment
  summarizing what the module's tests cover, matching the crate-wide
  `missing_docs` lint.
- No source module — test or production — may exceed 400 lines; split
  large modules by feature rather than suppressing the limit.

## Cargo and toolchain requirements

`Cargo.toml`'s `[lints.clippy]` block enables `clippy::pedantic` as a warning
tier and then denies a curated set of individual lints layered on top, covering
hygiene (`allow_attributes_without_reason`, `cognitive_complexity`), debugging
leftovers (`dbg_macro`, `print_stdout`, `print_stderr`), panic-prone operations
(`unwrap_used`, `expect_used`, `indexing_slicing`, `unreachable`), portability
(endian-specific byte conversions), numerical foot-guns (`float_arithmetic`, the
`cast_*` casts), and error-handling shape (`missing_panics_doc`,
`error_impl_error`, `result_large_err`). `[lints.rust]` denies `missing_docs`
and `[lints.rustdoc]` denies `missing_crate_level_docs`. `clippy.toml` tightens
`cognitive-complexity-threshold` to 9 and `too-many-arguments-threshold` to 4,
both well below Clippy's defaults, so functions that would pass a default
Clippy configuration can still fail `make lint` here.

In practice this means: every public item needs a doc comment (with an
`# Examples` block, by convention `rust,ignore` for anything that touches
process execution or the filesystem); lint suppressions must be scoped
`#[expect(clippy::..., reason = "...")]` (bare `#[allow(...)]` is itself
denied) and are used sparingly in this crate, for example around the two
`print_stdout` call sites in `src/lib.rs` where CLI output is the intended
behaviour; and helper functions should stay small and single-purpose to avoid
tripping the cognitive-complexity and argument-count ceilings — group related
parameters into a struct (see `PrLookup` in `src/status/mod.rs`) rather than
adding another positional parameter.

Dependencies are pinned with caret requirements. Notable runtime crates:
`camino`/`cap-std` (UTF-8, capability-oriented filesystem access in place of
`std::fs`/`std::path`), `rustix` (process-group signalling on Unix),
`wait-timeout` (bounding child-process execution), `directories` (XDG cache
resolution), `ortho_config` (layered CLI/env/config parsing), and `mockable`
(the `Clock` trait used to inject time). Dev-only crates (`rstest`,
`rstest-bdd`, `rstest-bdd-macros`, `assert_cmd`, `chrono`, `insta`, and
`tempfile`) back the three test layers above: `rstest` supplies fixtures and
parameterized unit tests; `rstest-bdd` and `rstest-bdd-macros` drive the
behavioural scenarios; `assert_cmd` invokes the binary in end-to-end tests;
`chrono` supplies deterministic timestamps for cache tests; `insta` records
rendered-output snapshots; and `tempfile` creates isolated test directories.
`mockall` generates the `MockCommandRunner` double described under
"Dependency-injection seams", and is mandatory for every seam it covers — a
bespoke, hand-rolled stub is not an acceptable substitute. `proptest` drives
the property-based tests in `src/install/property_tests.rs`.

### Real command execution

`RealCommandRunner::run` (in `command/mod.rs`) spawns the child in its own
process group on Unix (`rustix::process::kill_process_group`), and drains
stdout and stderr concurrently on dedicated reader threads while `wait_timeout`
waits for the child. This combination avoids two failure modes: a child that
writes more than the OS pipe buffer would otherwise block on `write` and be
misreported as a timeout, and a child that backgrounds a grandchild inheriting
the pipe write end would otherwise leave that descendant holding the pipe open,
so the reader threads would never observe EOF even after the direct child is
killed. On timeout, the whole process group is signalled, the reader threads
are joined to avoid leaking them, and `CommandError::Timeout` is returned as
the reported failure.

### Process-group signal ownership

`command::SignalClaim` coordinates the two bounded reader threads with the
owning `ChildSession`. Each participant shares one mutex-protected claim and
the child process ID captured at spawn time. The mutex is held through the
decision and the process-group `kill` syscall: recording a reap cannot race a
signal that has already been authorized but not yet delivered.

The claim has four states: `Unsignalled`, `Signalled`, `Reaped`, and `Sealed`.
Either a reader (`Signaller::Reader`) or the session (`Signaller::Session`) may
spend an `Unsignalled` claim, moving it to `Signalled`. When the direct child
is reaped, the claim moves to `Reaped` if no signal was sent, or to `Sealed` if
the group was already signalled. A session may spend a `Reaped` claim once,
moving it to `Sealed`, to evict a descendant that still holds a captured pipe
open. Readers cannot signal from `Reaped` or `Sealed`; this post-reap rule
prevents a reader from aiming a recycled process ID. A failed signal restores
the previous state so session cleanup can retry.
