# Follow-up issues

These issues describe work that is explicitly out of scope for the current
pull request. They are recorded here so the rationale and acceptance criteria
survive independently of any single review thread.

## Issue A: separate domain policy from adapters

### Context

Several modules currently mix domain policy with the concerns of a specific
adapter:

- `config::StatusArgs` and `config::InstallArgs` derive both `clap::Parser`
  and `ortho_config::OrthoConfig`/`serde::Deserialize`, so CLI parsing
  attributes (`#[arg(long)]`) travel with the struct into every function
  that consumes it, including `status::build_status_line`.
- `error::DbarError` has a `Config` variant that wraps
  `std::sync::Arc<ortho_config::OrthoError>` directly, so a caller matching
  on `DbarError` is coupled to the `ortho_config` crate's error type rather
  than a dbar-owned abstraction.
- `types::StatusPosition` encodes tmux vocabulary (`status-left`,
  `status-right`) inside a type that is otherwise a general "left or right"
  domain enum, and `install::build_snippet` is the only consumer that needs
  tmux-specific strings.
- `status::resolve_project_dir` calls `std::env::current_dir()` directly
  instead of receiving the working directory through an injected
  dependency, which makes the fallback path untestable without mutating
  process-wide state.
- `cache::load_cached_value` and `cache::store_cached_value` combine cache
  policy (TTL comparison, key resolution) with JSON serialization
  (`serde_json`) and `cap_std`-backed file persistence in the same
  functions, so a policy change (for example, a different eviction rule)
  cannot be tested without also exercising the filesystem.
- `render::render_status_line` and its segment helpers
  (`render_branch_segment`, `render_pr_segment`, `render_tmux_segment`) are
  tmux-specific — they emit `#[fg=colour...]` styling and `#{...}`/`#(...)`
  escaping — with no separation between "what to display" and "how tmux
  renders it".

`src/command.rs` already models the adapter boundary reasonably well: the
`CommandRunner` trait separates the domain need ("run this command and
capture its output") from `RealCommandRunner`'s process-spawning
implementation, and `src/github.rs`'s `GitHubClient` trait does the same for
PR lookups (`GhCliClient` versus `MockGitHubClient`). Both are useful
reference points for what a clean boundary looks like elsewhere in the
crate.

### Proposal

Introduce explicit ports for the adapters that are currently reached into
directly from domain code, following the pattern already established by
`CommandRunner` and `GitHubClient`:

- A working-directory port so `status::resolve_project_dir` depends on an
  injected abstraction rather than calling `std::env::current_dir()`
  directly.
- A cache-storage port that separates TTL/key policy (currently inline in
  `status::pr_number` and `cache::load_cached_value`) from the JSON/serde
  encoding and `cap_std` file operations that back it.
- A render port that separates the domain-level `render::RenderContext`
  (project, git status, PR number, clock) from a tmux-specific renderer,
  so a future non-tmux consumer (or a snapshot test) does not need to
  parse `#[...]` escape sequences.
- A dbar-owned configuration-error type in `error::DbarError` that does not
  expose `ortho_config::OrthoError` (or the `Arc` wrapper) across the
  module boundary.

### Acceptance criteria

1. `status::resolve_project_dir` no longer calls
   `std::env::current_dir()` directly; it accepts the working directory (or
   a trait object that supplies it) as a parameter, and a unit test can
   exercise the "no `--project-dir` supplied" fallback without depending on
   the test process's actual working directory.
2. Cache TTL and key-resolution logic (currently in `status::pr_number`
   and `cache::load_cached_value`) can be unit-tested against an in-memory
   store, without going through `cap_std::fs_utf8::Dir` or writing to a
   temporary directory.
3. `render::render_status_line`'s domain inputs (`RenderContext`) remain
   free of tmux escape-sequence concerns; a unit test exists that asserts
   the domain layer produces plain segment data, and a separate test
   confirms the tmux renderer performs the `#`-doubling described in
   `render::escape_tmux`'s doc comment.
4. `error::DbarError` no longer has a variant whose wrapped type is
   `std::sync::Arc<ortho_config::OrthoError>`; `config::load_command`'s
   public signature (or its caller in `lib::run`) maps ortho_config errors
   into a dbar-owned error variant instead.
5. `types::StatusPosition` either drops its tmux-specific `Display` strings
   (`"left"`/`"right"` are acceptable as domain values) or the mapping from
   `StatusPosition` to tmux's `status-left`/`status-right` target names
   moves into `install::build_snippet`, which is its only consumer.
6. Existing behaviour is unchanged: `cargo test` passes with no
   modification to observable CLI output, `install` snippet contents, or
   cache file format.
7. Each new port (working directory, cache storage, render) is expressed as
   a trait with a single production implementation, mirroring
   `command::CommandRunner` and `github::GitHubClient`, so tests can supply
   a stub without an integration harness.

### Out of scope

- Adding new CLI flags, configuration keys, or subcommands.
- Changing the on-disk cache file format or the tmux snippet emitted by
  `install::build_snippet`.
- Introducing a plugin system for alternative status-bar backends (for
  example, a non-tmux renderer); this issue only asks for the render port
  to be separable, not for a second implementation to be built.

### Risks and trade-offs

`dbar` is a small, single-binary crate (fewer than a dozen modules, per
`src/lib.rs`'s `mod` list) with one production implementation for each
existing port. Introducing additional trait boundaries purely for
separation's sake risks adding indirection — extra trait objects, extra
constructor parameters threaded through `status::build_status_line` and
`status::pr_number` — without a second implementation ever appearing to
justify it. The migration should be judged against concrete testability
gains (each acceptance criterion above ties a port to a specific test that
is currently hard or impossible to write), not against abstraction for its
own sake. If a given port does not unlock a genuinely new test, it should
be dropped from the migration rather than added speculatively.

Suggested migration order, smallest and lowest-risk first:

1. `error::DbarError`'s `ortho_config` variant (isolated, no call-site
   fan-out).
2. The working-directory port for `status::resolve_project_dir` (single
   call site).
3. The render port, since `render/mod.rs` already separates concerns
   reasonably well internally.
4. The cache-storage port last, since it touches the most call sites
   (`status::pr_number`, `cache::resolve_cache_dir`, and their tests).

## Issue B: structured diagnostics and telemetry

### Context for telemetry

There is currently no logging, tracing, or metrics anywhere in the crate;
`Cargo.toml` lists no `tracing`, `log`, or metrics dependency, and the only
diagnostic surface is the `Debug`/`Error` implementations on
`command::CommandError`, `github::GitHubError`, `cache::CacheError`, and
`install::InstallError`.

Several failure paths are deliberately swallowed rather than surfaced:

- `status::pr_number` discards cache-write failures with
  `if let Err(_err) = cache::store_cached_value(...) {}`.
- `status::pr_number` treats a failed GitHub lookup
  (`Err(_err) => return pr_from_branch(context.branch)`) as silent fallback
  to branch-name parsing.
- `git::is_git_repo`, `git::git_branch`, `git::git_worktree_status`, and
  `git::upstream_counts` all use `Ok(...) ... _ => default` patterns that
  discard the underlying `command::CommandError`.
- `tmux::resolve_context` discards the result of `runner.run(&spec)` via
  `.ok()` in `query_tmux`.

`Cargo.toml`'s `[lints.clippy]` section denies both `print_stdout` and
`print_stderr`, and `run_status` in `src/lib.rs` already carries an
`#[expect(clippy::print_stdout, ...)]` annotation to print the status line.
Because `dbar status` runs on every tmux status-bar refresh (typically every
few seconds, as configured via `status-interval` in tmux, with the command
line built by `install::build_snippet`), any added instrumentation must add
negligible latency and must not compete with the status line for stdout,
which is dedicated to the rendered segment consumed by tmux's `#(...)`.

### Proposed instrumentation

Add structured diagnostics at the boundaries where the crate already talks
to external systems: `command::CommandRunner::run` (git/gh/tmux process
execution), `github::GitHubClient::pr_number`, `git::git_status`,
`tmux::resolve_context`, `cache::load_cached_value`/`store_cached_value`,
and `install::install`. Diagnostics should be gated behind an opt-in mode so
the default `dbar status` invocation, run every few seconds from tmux,
carries no added overhead.

### Acceptance criteria for telemetry

1. A `--verbose` flag (or equivalent diagnostic-mode flag) is added to
   `config::StatusArgs` and/or `config::InstallArgs`, off by default, that
   enables structured diagnostic output without changing the rendered
   status line or `install` outcome messages.
2. Diagnostic output is written to stderr or an explicitly configured log
   file, never to stdout, so it cannot corrupt the tmux status segment that
   `run_status` prints via `println!`; any implementation must reconcile
   this with the crate's `clippy::print_stderr` lint (denied in
   `Cargo.toml`), for example by using a logging/tracing crate rather than
   bare `eprintln!`, or by scoping a documented `#[expect(...)]` the way
   `run_status` already does for `print_stdout`.
3. Counters record command outcomes (success, non-zero exit, timeout) per
   external tool at a bounded cardinality: keyed by a small fixed set of
   labels such as program name (`git`, `gh`, `tmux`) and outcome kind
   (`success`, `CommandError::NonZero`, `CommandError::Timeout`), not by
   unbounded values such as branch names, project directories, or PR
   numbers.
4. Latency is measured around each external boundary call: at minimum,
   `command::RealCommandRunner::run`, `github::GhCliClient::pr_number`,
   `cache::load_cached_value`, `cache::store_cached_value`, and
   `install::install`'s filesystem operations.
5. Cache-write failures currently discarded in `status::pr_number`
   (`if let Err(_err) = cache::store_cached_value(...) {}`) and failed
   GitHub lookups (`Err(_err) => return pr_from_branch(...)`) are recorded
   as diagnostic events when diagnostics are enabled, without changing the
   existing fallback behaviour on the non-diagnostic path.
6. A benchmark or timed test demonstrates that enabling diagnostics adds no
   more than a small, explicitly stated overhead (for example, low
   single-digit milliseconds) to a representative `dbar status` run, and
   that the default (diagnostics disabled) path shows no measurable
   regression against the current baseline.
7. Diagnostic instrumentation does not introduce new `unwrap`, `expect`, or
   `panic!` paths, in line with the crate's existing
   `clippy::unwrap_used`/`clippy::expect_used`/`clippy::panic_in_result_fn`
   deny lints.
8. Existing `CommandError`, `GitHubError`, `CacheError`, and `InstallError`
   variants and their `Display` messages are unchanged, so this work adds
   an observability layer without altering the crate's existing error
   contracts.

### Out of scope for telemetry

- Shipping a metrics exporter, dashboard, or remote telemetry sink; the
  proposal covers local structured diagnostics and in-process counters
  only.
- Changing default CLI behaviour: with diagnostics disabled, `dbar status`
  output and exit codes must remain exactly as they are today.
- Replacing the existing `thiserror`-based error types
  (`command::CommandError`, `github::GitHubError`, `cache::CacheError`,
  `install::InstallError`) with a different error-handling strategy.

### Risks and trade-offs for telemetry

Because `dbar status` is invoked on a tight refresh interval from tmux
(`status-interval`), any instrumentation that allocates, locks, or performs
I/O on the hot path risks visibly slowing the status bar, which is the
crate's primary purpose. High-cardinality labels (branch names, PR numbers,
project paths) must be avoided in counters, both to bound memory use over a
long-running tmux session and to avoid leaking directory or branch names
into log output that might be captured more broadly than intended. The
existing `clippy::print_stdout`/`clippy::print_stderr` deny lints mean any
naive `eprintln!`-based logging will fail the commit gate; adopting a
tracing/logging crate is a new dependency decision that should be weighed
against the crate's currently minimal dependency footprint (`Cargo.toml`
lists no logging crate today).
