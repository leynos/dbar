# Developers' guide

This guide orients contributors working on `dbar`, a CLI that renders a
tmux-ready status segment and installs the tmux configuration snippet that
invokes it. It covers module responsibilities, the dependency-injection seams
used for hermetic testing, the tooling gates enforced in CI, and the test
layers and lint constraints that shape test code.

## Architecture overview

`src/lib.rs` is the crate root. It declares the module tree and exposes two
entry points invoked from `main`: `run_status` and `run_install`, dispatched
from `run()` based on the parsed `DbarCommand`.

- `config.rs` — CLI parsing (`clap`) and configuration merging
  (`ortho_config`). `Cli`/`Commands` define the `status` and `install`
  subcommands; `StatusArgs` and `InstallArgs` are `OrthoConfig` structs
  merged from CLI flags, environment variables (`DBAR_` prefix), and
  configuration files. `load_command()` returns a `DbarCommand`.
- `command.rs` — the `CommandRunner` trait and `CommandSpec` builder used to
  shell out to `git`, `gh`, and `tmux`. See "Dependency-injection seams"
  below.
- `git.rs` — free functions `project_name` and `git_status` that run `git`
  probes through a `&dyn CommandRunner` and parse the results into
  `ProjectName` and `GitStatus`.
- `github.rs` — the `GitHubClient` trait and its two implementations,
  `GhCliClient` (backed by the `gh` CLI via `CommandRunner`) and
  `MockGitHubClient` (a fixed value, wired up when `--github-mock-pr` is
  supplied).
- `tmux.rs` — `TmuxContext` and `resolve_context`, which fills in missing
  session/window/pane/socket fields by querying `tmux display-message`
  through a `&dyn CommandRunner`.
- `cache/mod.rs` — resolves the XDG cache directory (via `directories`) and
  performs TTL-checked reads and atomic temp-file-then-rename writes of
  cached PR lookups, plus the bounded retention sweep described under "Cache
  retention" below.
- `status.rs` — `build_status_line` orchestrates a single status line: it
  resolves the project directory, probes git, looks up (and caches) the PR
  number, resolves tmux context, renders the clock label, and passes
  everything to `render::render_status_line`.
- `render/mod.rs` — pure rendering: `RenderContext` plus `render_status_line`
  assemble the tmux `#[...]` style tags and Powerline-style glyphs into the
  final string, with optional right-alignment to a client width.
- `types.rs` — domain newtypes (`ProjectName`, `BranchName`, `AheadCount`,
  `BehindCount`, `PrNumber`, `CacheTtlSeconds`, `StatusPosition`) that avoid
  passing bare `String`/integer values between modules.
- `install/mod.rs` — `install()` inserts or updates a marker-delimited tmux
  snippet in a configuration file, using `cap_std`/`camino` for path-capable,
  UTF-8-only filesystem access, and backs up the previous contents before
  replacing the target atomically: the new contents are written to a
  uniquely named temporary file, which inherits the target's permissions,
  then renamed over the target.
- `error.rs` — `DbarError`, the top-level error enum returned by `run()`,
  wrapping `CacheError`, `OrthoError`, `InstallError`, and `std::io::Error`.

### End-to-end assembly of a status line

1. `run_status` in `src/lib.rs` constructs a `RealCommandRunner`, a
   `DefaultClock` (from `mockable`), and either a `GhCliClient` or a
   `MockGitHubClient` depending on whether `--github-mock-pr` was passed.
2. `status::build_status_line` resolves the project directory (from
   `--project-dir` or the current working directory), then calls
   `git::project_name` and `git::git_status`.
3. If PR lookup is enabled and a git branch was found, `pr_number` first
   checks the on-disk cache (via `cache::load_cached_value`), then falls back
   to `GitHubClient::pr_number`, then to a `pr/<n>`-style branch-name
   heuristic. A successful non-empty result is written back through
   `cache::store_cached_value`; a failed lookup is never cached, so a
   transient network error does not poison the PR value for the whole TTL.
4. `tmux::resolve_context` fills in any tmux fields not already supplied on
   the command line by querying `tmux display-message`.
5. `render::render_status_line` combines the project, git, PR, tmux, and
   clock segments into the final tmux-ready string, which `run_status` prints
   to stdout.

### Cache retention

Each `(project directory, branch)` pair hashes to its own
`pr_<16 hex digits>.json` file, so entries would otherwise accumulate for
every branch a checkout has ever had, including branches long since deleted.
`cache/mod.rs` reclaims them under this policy:

- **Trigger.** Only a read that finds an entry past its TTL sweeps the
  directory. That path is already committed to a fresh `gh` lookup, so the
  common cache hit — the one taken on every tmux refresh — never lists the
  directory at all. Writes never sweep, because `store_cached_value` is given
  no TTL to judge entries by.
- **Bound.** One sweep lists at most 256 names, opens and parses at most 16
  of them, and removes at most 8 files. A backlog is therefore cleared across
  successive runs rather than in one unbounded pass on the hot path.
- **Ownership.** The cache directory may be shared, so a file is removed only
  when all three of these hold: its name is `pr_` followed by exactly 16
  lowercase hex digits and `.json`; it is a regular file whose contents
  deserialize as a cache entry; and that entry's own recorded timestamp puts
  it past the TTL. Anything merely named like an entry — `pr_deadbeef.json`,
  `pr_0123456789abcdef.json.tmp`, uppercase hex, or an identically named
  directory — is left alone, as is any file whose contents do not parse.

Expiry is judged from the entry's recorded timestamp rather than from file
metadata, so an entry another dbar process has just refreshed reads as fresh.
A file that vanishes between the listing and the removal counts as success:
another process reclaimed it first. Any other removal failure is returned as
`CacheError::Retention` in place of the `Ok(None)` the expiry would otherwise
have produced, so the caller can log it and carry on with a fresh lookup
rather than the failure being discarded silently.

## Dependency-injection seams

`dbar` shells out to `git`, `gh`, and `tmux`, and reads the wall clock. Tests
must exercise the parsing and orchestration logic without invoking real
processes or mutating the environment, so two trait boundaries carry all
external process interaction, and the `mockable` crate's `Clock` trait
carries time:

- `command::CommandRunner` — `fn run(&self, spec: &CommandSpec) ->
  Result<CommandOutput, CommandError>`. `RealCommandRunner` executes real
  processes (with a timeout, process-group termination, and concurrent pipe
  draining; see "Real command execution" below). Tests provide a stub that
  implements `CommandRunner` and returns canned `CommandOutput`s or errors
  for known `CommandSpec`s.
- `github::GitHubClient` — `fn pr_number(&self, project_dir: &Utf8Path,
  branch: &str) -> Result<Option<PrNumber>, GitHubError>`. `GhCliClient`
  wraps a `&dyn CommandRunner` to shell out to `gh`; `MockGitHubClient`
  returns a fixed, pre-configured PR number and is the concrete type wired
  up for `--github-mock-pr`, but any test can implement the trait directly
  for finer control (see `FailingGitHubClient` in `src/status.rs`).

Both traits exist so unit and behavioural tests stay hermetic: no test needs
network access, a real `git`/`gh`/`tmux` binary, or environment-variable
mutation to exercise the orchestration logic in `git.rs`, `tmux.rs`, and
`status.rs`.

`src/git.rs` and `src/tmux.rs` each define a private `StubRunner` in their
`#[cfg(test)] mod tests` block as a worked example of a `CommandRunner`
double: `git.rs`'s stub maps exact `CommandSpec` values to canned stdout via
a `HashMap`, while `tmux.rs`'s stub returns one canned response (or an
error) and counts how many times it was called, to prove short-circuiting.
New tests needing a `CommandRunner` double should follow one of these two
patterns rather than introducing a new abstraction.

## Tooling and gates

The `Makefile` wraps the commands contributors are expected to run before
committing:

- `make build` / `make release` — build the debug or release binary.
- `make test` — `cargo test --all-targets --all-features` with `-D
  warnings`.
- `make lint` — runs `cargo doc --no-deps`, then `cargo clippy
  --all-targets --all-features -- -D warnings`, then the Whitaker Dylint
  suite (`whitaker --all`) against the same targets and features, and
  finally the spelling gate (see below).
- `make fmt` — runs `cargo fmt --all` and `mdformat-all`.
- `make check-fmt` — verifies formatting without modifying files
  (`cargo fmt --all -- --check`).
- `make markdownlint` — runs `markdownlint-cli2` over every Markdown file,
  then the spelling gate.
- `make spelling` — regenerates `typos.toml` from
  `scripts/generate_typos_config.py` and runs the pinned `typos` release
  against every tracked Markdown file, enforcing en-GB-oxendict spelling.
- `make nixie` — validates Mermaid diagrams embedded in Markdown files.

`typos.toml` is generated output; it is regenerated by `make spelling` (and
therefore by `make lint` and `make markdownlint`) from the spelling policy
under `scripts/`. Repository-only exceptions belong in `typos.local.toml`;
hand-editing `typos.toml` is not supported and any edits are overwritten on
the next generation run.

## Testing guidance

Tests are organized in three layers:

1. Unit tests — `#[cfg(test)] mod tests` blocks colocated with the module
   under test (for example `src/command.rs`, `src/cache/mod.rs` and
   `src/cache/tests.rs`, `src/git.rs`,
   `src/tmux.rs`, `src/render/mod.rs`, `src/status.rs`,
   `src/install/mod.rs`/`src/install/tests.rs`). Cases use `#[rstest]`, with
   `#[case]` parameterization for table-style coverage and `#[fixture]` for
   shared setup such as temporary directories.
2. Behavioural tests — `rstest-bdd` scenarios under `tests/rstest_bdd/`,
   driven by a `.feature` file (`status.feature`) and step implementations
   in `status_steps.rs`, loaded from the `tests/rstest_bdd_tests.rs` crate
   root.
3. End-to-end snapshot tests — `tests/e2e/`, using `assert_cmd` to invoke
   the built `dbar` binary against a temporary git repository and `insta`
   to snapshot the rendered status line, loaded from `tests/e2e_tests.rs`.

### Lint constraints that shape test code

The workspace-wide Clippy profile (`Cargo.toml`'s `[lints.clippy]`) denies
`unwrap_used` and `expect_used` everywhere, but `clippy.toml` sets
`allow-expect-in-tests = true`, which narrows that allowance to `#[test]`
and `#[rstest]` function bodies specifically. In practice, for files under
`src/`:

- `expect(...)` is permitted only inside a `#[test]`/`#[rstest]` function
  body, never inside a `#[fixture]` body or a plain helper function, even
  one only ever called from tests. Fixtures and helpers must return
  `Result` and propagate failures with `?`, or use an explicit `panic!`
  with a clear message when a `Result` return is not possible (for example
  in an `rstest-bdd` `#[fixture]`, whose return type is fixed by the
  framework; see `world()` in `tests/rstest_bdd/status_steps.rs`).
- Every `#[cfg(test)] mod tests` block needs a `//!` inner doc comment
  summarizing what the module's tests cover, matching the crate-wide
  `missing_docs` lint.
- No source module — test or production — may exceed 400 lines; split
  large modules by feature rather than suppressing the limit.

## Cargo and toolchain requirements

`Cargo.toml`'s `[lints.clippy]` block enables `clippy::pedantic` as a
warning tier and then denies a curated set of individual lints layered on
top, covering hygiene (`allow_attributes_without_reason`,
`cognitive_complexity`), debugging leftovers (`dbg_macro`, `print_stdout`,
`print_stderr`), panic-prone operations (`unwrap_used`, `expect_used`,
`indexing_slicing`, `unreachable`), portability (endian-specific byte
conversions), numerical foot-guns (`float_arithmetic`, the `cast_*` casts),
and error-handling shape (`missing_panics_doc`, `error_impl_error`,
`result_large_err`). `[lints.rust]` denies `missing_docs` and
`[lints.rustdoc]` denies `missing_crate_level_docs`. `clippy.toml` tightens
`cognitive-complexity-threshold` to 9 and `too-many-arguments-threshold` to
4, both well below Clippy's defaults, so functions that would pass a
default Clippy configuration can still fail `make lint` here.

In practice this means: every public item needs a doc comment (with an
`# Examples` block, by convention `rust,ignore` for anything that touches
process execution or the filesystem); lint suppressions must be scoped
`#[expect(clippy::..., reason = "...")]` (bare `#[allow(...)]` is itself
denied) and are used sparingly in this crate, for example around the two
`print_stdout` call sites in `src/lib.rs` where CLI output is the intended
behaviour; and helper functions should stay small and single-purpose to
avoid tripping the cognitive-complexity and argument-count ceilings — group
related parameters into a struct (see `PrLookup` in `src/status.rs`) rather
than adding another positional parameter.

Dependencies are pinned with caret requirements. Notable runtime crates:
`camino`/`cap-std` (UTF-8, capability-oriented filesystem access in place of
`std::fs`/`std::path`), `rustix` (process-group signalling on Unix),
`wait-timeout` (bounding child-process execution), `directories` (XDG cache
resolution), `ortho_config` (layered CLI/env/config parsing), and
`mockable` (the `Clock` trait used to inject time). Dev-only crates
(`rstest`, `rstest-bdd`, `rstest-bdd-macros`, `assert_cmd`, `insta`,
`tempfile`) back the three test layers above.

### Real command execution

`RealCommandRunner::run` (in `command.rs`) spawns the child in its own
process group on Unix (`rustix::process::kill_process_group`), and drains
stdout and stderr concurrently on dedicated reader threads while
`wait_timeout` waits for the child. This combination avoids two failure
modes: a child that writes more than the OS pipe buffer would otherwise
block on `write` and be misreported as a timeout, and a child that
backgrounds a grandchild inheriting the pipe write end would otherwise
leave that descendant holding the pipe open, so the reader threads would
never observe EOF even after the direct child is killed. On timeout, the
whole process group is signalled, the reader threads are joined to avoid
leaking them, and `CommandError::Timeout` is returned as the reported
failure.
