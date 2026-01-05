# Implement tmux status bar for dbar

This ExecPlan is a living document. The sections `Constraints`, `Tolerances`,
`Risks`, `Progress`, `Surprises & Discoveries`, `Decision Log`, and
`Outcomes & Retrospective` must be kept up to date as work proceeds.

Status: COMPLETE

No `PLANS.md` exists in this repository, so this document is the source of
truth for the plan.

## Purpose / Big Picture

Deliver a `dbar` CLI that prints a tmux-ready status segment showing the
current project name, git branch, git working tree state, GitHub PR,
ahead/behind counts, worktree status, and tmux session/pane/socket information.
Provide an `install` subcommand that adds the required tmux directives to a
user’s tmux configuration, and cache expensive lookups in the XDG cache
directory. The output must borrow the colours and glyphs from
`~/.local/bin/claude-status` while using `ortho_config` for configuration.
Success means `dbar` renders a stable, themed status line that can be embedded
in tmux via `#(dbar)` and all specified tests pass.

## Constraints

- Must use `ortho_config` for configuration loading and CLI parsing.
- Must borrow palette and glyphs from `~/.local/bin/claude-status`, including
  the GitHub glyph `f408` for PR output.
- Must follow `docs/tmux-statuslines-in-a-nutshell.md`, emitting tmux style
  tags `#[...]` instead of ANSI escapes and using tmux formats (`#{...}`) or
  tmux queries to collect context.
- Must include unit tests with `rstest`, behavioural tests with `rstest-bdd`,
  and e2e snapshot tests with `assert-cmd` + `insta`.
- Must use `cap_std` and `camino` in place of `std::fs` and `std::path`.
- Must use the XDG cache directory (via the `directories` crate) for caching.
- Every new module must start with a `//!` module-level doc comment and stay
  under 400 lines.
- No environment mutation in tests unless guarded via `mockable` or shared
  locks; prefer dependency injection.
- Use Makefile targets for validation and capture long outputs with `tee`.
- Use en-GB-oxendict spelling in docs and comments.

## Tolerances (Exception Triggers)

- Scope: if implementation requires changes to more than 12 files or more than
  900 net new lines, stop and ask for guidance.
- Dependencies: if additional crates are required beyond the ones listed in
  `Interfaces and Dependencies`, stop and ask before adding them.
- Interface: if the CLI contract must expose subcommands beyond `status` and
  `install`, stop and ask.
- Iterations: if tests still fail after three fix attempts, stop and ask.
- Ambiguity: if `claude-status` aesthetics or tmux format needs cannot be
  matched without choosing a different output format (ANSI vs tmux style), stop
  and ask with options.
- Config mutation: if install needs to edit a tmux config file without an
  explicit user-supplied path or an agreed default, stop and ask.

## Risks

- Risk: tmux does not render ANSI colour escape codes consistently.
  Severity: medium Likelihood: medium Mitigation: emit tmux
  `#[fg=colourNN]`/`#[bg=colourNN]` styling using the same 256-colour palette
  as `claude-status`.

- Risk: `gh` or `git` commands are unavailable or slow in status refresh.
  Severity: medium Likelihood: medium Mitigation: treat command failures as
  empty values and keep output stable; expose a config toggle to skip PR lookup.

- Risk: worktree detection logic differs from `claude-status` expectations.
  Severity: low Likelihood: medium Mitigation: implement the same `.worktrees`
  path heuristic and add explicit tests for main/worktree paths.

- Risk: modifying tmux configuration could be destructive or non-idempotent.
  Severity: medium Likelihood: medium Mitigation: add markers to inserted
  sections, support `--dry-run`, and back up files before writing.

- Risk: cached GitHub PR lookups become stale or invalid.
  Severity: low Likelihood: medium Mitigation: keep cache TTL short and allow
  bypass via config.

## Progress

- [x] (2026-01-05 00:00Z) Captured `claude-status` palette, glyphs, and
  formatting behaviour.
- [x] (2026-01-05 00:00Z) Reviewed tmux status line protocol guidance.
- [x] (2026-01-05 00:00Z) Plan approved; implementation started.
- [x] (2026-01-05 00:00Z) Designed configuration schema and CLI contract.
- [x] (2026-01-05 00:00Z) Built core status model and renderer scaffolding.
- [x] Add unit, BDD, and e2e snapshot tests.
- [x] Update documentation for tmux usage and configuration.
- [x] Run formatting, lint, and test gates.
- [x] Commit changes.

## Surprises & Discoveries

- Observation: running `make fmt` reformats multiple existing docs and
  `src/main.rs`, creating unrelated diffs. Evidence: `git status --short` shows
  changes beyond the new plan file. Impact: keep the formatting changes to
  avoid manual reverts.

- Observation: `make fmt` failed due to Markdown lint errors in
  `docs/tmux-statuslines-in-a-nutshell.md`. Evidence:
  `MD036/no-emphasis-as-heading` errors for two bolded lines. Impact: convert
  the bolded lines to headings before proceeding.

- Observation: cargo registry writes required elevated permissions.
  Evidence: `cargo check` failed with permission errors until rerun with
  escalated access to `/home/leynos/.cargo`. Impact: use escalated permissions
  for build and test gates if needed.

- Observation: integration tests under nested `tests/` directories were not
  discovered by Cargo. Evidence: `cargo test` only ran unit tests until
  top-level test crates were added. Impact: add `tests/e2e_tests.rs` and
  `tests/rstest_bdd_tests.rs` to load the submodules.

- Observation: the CLI treated `pr_cache_ttl_seconds` as required after adding
  explicit long flags. Evidence: e2e tests failed with a missing
  `--pr-cache-ttl-seconds` error. Impact: set a clap default value for the
  field.

## Decision Log

- Decision: plan to emit tmux style codes (not ANSI) while using the same
  palette values as `claude-status`. Rationale: tmux renders its own style
  markup reliably in status lines. Date/Author: 2026-01-05 / Codex

- Decision: add an `install` subcommand that injects tmux config directives and
  use the XDG cache directory via `directories` for caching. Rationale: aligns
  with user request and tmux performance guidance. Date/Author: 2026-01-05 /
  Codex

- Decision: keep the formatting changes introduced by `make fmt` and commit
  them alongside the plan updates. Rationale: keeps the docs aligned with
  formatting requirements and avoids manual rollback. Date/Author: 2026-01-05 /
  Codex

- Decision: add an explicit `clap` dependency to satisfy `ortho_config`'s
  generated CLI parsing requirements. Rationale: the derive macros require
  `clap` to be available in the crate. Date/Author: 2026-01-05 / Codex

- Decision: add explicit clap long flags and a default for
  `pr_cache_ttl_seconds`. Rationale: keep the CLI optional arguments optional
  and avoid missing-argument failures in tests. Date/Author: 2026-01-05 / Codex

- Decision: wire tmux `pane_current_path` into the install snippet to scope
  git probes to the active pane directory. Rationale: ensures branch and
  project context match the shell running inside tmux. Date/Author: 2026-01-05
  / Codex

## Outcomes & Retrospective

Implemented a tmux status line renderer with install support, git metadata, PR
lookup caching, and tmux context probing. Added unit tests, rstest-bdd
behavioural coverage, and insta snapshot tests. Updated documentation and
validated formatting, linting, and tests. Next time, scaffold integration test
crate roots earlier to avoid missing coverage during initial test runs.

## Context and Orientation

The repository currently contains a single binary with a stub `main` in
`src/main.rs`. There are no existing modules or tests. The aesthetic reference
script `~/.local/bin/claude-status` defines glyphs, a 256-colour palette, and
segment ordering; it also contains project naming and git parsing logic that
must be mirrored. The tmux status line protocol and quoting guidance live in
`docs/tmux-statuslines-in-a-nutshell.md`. The configuration system should use
`ortho_config` as documented in `docs/ortho-config-users-guide.md`. Behavioural
test patterns are in `docs/rstest-bdd-users-guide.md`, and dependency-injection
expectations are in `docs/reliable-testing-in-rust-via-dependency-injection.md`.

## Plan of Work

Stage A: confirm requirements and desired output. Read
`~/.local/bin/claude-status`, capture glyphs, palette numbers, and segment
structure. Read `docs/tmux-statuslines-in-a-nutshell.md` to align with tmux
format, style, and command-substitution rules. Decide which elements map to
(tmux) style codes, and whether tmux context is passed as arguments or queried
internally.

Stage B: scaffolding and tests. Introduce a small library layout with modules
for configuration, git probing, tmux probing, install logic, caching, and
rendering. Add a public API (`dbar::status::build_status_line`) that accepts
injected dependencies for process, environment, and time. Create fixtures and
helper builders for tests using `rstest` and `mockable`.

Stage C: implementation. Implement project name detection, git status parsing
(staged, dirty, ahead, behind), PR detection via `gh` with branch-name
fallback, worktree detection, and tmux metadata parsing. Implement caching of
expensive lookups in the XDG cache directory. Implement a renderer that emits
segments with the `claude-status` palette and glyphs, replacing the PR glyph
with `f408`. Wire config through `ortho_config` so CLI/env/config can override
values such as `project_dir`, `show_pr`, `style`, and cache TTL. Implement the
`install` subcommand to add tmux directives with markers and idempotence.

Stage D: documentation and cleanup. Add a usage section to a doc (likely
`docs/users-guide.md`) describing tmux configuration, the `install` subcommand,
and config keys. Ensure all lint, format, and test gates pass and commit each
logical change.

Each stage ends with validation before moving on.

## Concrete Steps

All commands run from `/data/leynos/Projects/dbar`.

1. Inspect palette and glyphs in `~/.local/bin/claude-status`.
   - If reading outside the workspace fails, request permission.

2. Create module layout (example layout, adjust as needed):

    src/lib.rs
    src/config.rs
    src/git.rs
    src/tmux.rs
    src/render.rs
    src/status.rs
    src/install.rs
    src/cache.rs

3. Add dependencies (exact versions via `cargo search`):

    - ortho_config
    - serde + serde_json
    - camino
    - cap-std
    - thiserror (typed errors)
    - directories
    - mockable (dev)
    - mockall (dev)
    - rstest (dev)
    - rstest-bdd + rstest-bdd-macros (dev)
    - assert_cmd + insta (+ predicates if needed) (dev)
    - tempfile (dev)

4. Add tests:
   - Unit tests in `src/*` using `rstest` for parsing and rendering.
   - Behavioural tests in `tests/rstest_bdd/` with `.feature` files that
     describe clean/dirty repos, ahead/behind, and PR detection.
   - E2E snapshot tests in `tests/e2e/` using `assert_cmd` + `insta` against
     temp git repos with deterministic fixtures.

5. Implement install logic:
   - Define a tmux snippet using `#(dbar status ...)` and pass tmux formats as
     arguments.
   - Update a user-selected tmux config file with start/end markers.
   - Provide `--dry-run` and `--path` options for safety.

6. Implement main binary:
   - Load config via `OrthoConfig`.
   - Collect dependencies (real env, command runner).
   - Print status line to stdout; exit 0 even when git/tmux data missing.

7. Update documentation:
   - Add tmux config snippet and config keys.
   - Run `make fmt` and `make markdownlint` after doc changes.

8. Gate and commit each atomic change:
   - `make check-fmt | tee /tmp/dbar-check-fmt.log`
   - `make lint | tee /tmp/dbar-lint.log`
   - `make test | tee /tmp/dbar-test.log`

## Validation and Acceptance

Behavioural acceptance:

- Running `dbar` inside a git repo produces a status line containing:
  project name, branch, staged/dirty/clean glyphs, ahead/behind counts, PR
  number prefixed by the GitHub glyph `f408`, worktree indicator, and tmux
  session/pane/socket data if available.
- Running `dbar` outside a git repo still produces a project segment using the
  current directory name and omits git-specific segments gracefully.
- The output uses the colour palette and glyphs from `claude-status`.
- Running `dbar install --path <tmux.conf>` inserts an idempotent snippet that
  adds `#(dbar status ...)` to the configured status location without
  clobbering unrelated tmux config.

Quality criteria:

- Tests: `make test` passes, including new rstest, rstest-bdd, and e2e snapshot
  suites. The new tests fail before implementation and pass after.
- Lint/typecheck: `make lint` passes with no warnings.
- Formatting: `make check-fmt` passes; `make fmt` and `make markdownlint` pass
  when docs are touched.

## Idempotence and Recovery

All steps are additive. If a step fails, fix the issue and re-run the same
command. Avoid destructive git operations. Keep temporary test repos confined
to temp directories created per test and cleaned automatically by `tempfile`.

## Artifacts and Notes

Expected tmux usage snippet (final doc should include something like this):

    set -g status-right '#(dbar status "#{session_name}" "#{window_index}" "#{pane_id}")'

Example status output (illustrative; exact values depend on repo state):

    <project segment> <branch segment> <pr segment> <tmux segment>

## Interfaces and Dependencies

Proposed core interfaces (final names may adjust to fit codebase):

- `dbar::config::DbarConfig`: `#[derive(OrthoConfig)]` struct with fields
  `project_dir: Option<Utf8PathBuf>`, `show_pr: bool`, `style: StyleMode`, and
  `tmux_format: TmuxFormat` (or similar), plus cache settings.
- `dbar::status::build_status_line(ctx: &StatusContext) -> StatusLine`:
  returns a rendered string or a struct with `render()`.
- `dbar::git::GitProbe`: trait with methods to read branch, status, upstream
  counts, and worktree detection, with a real implementation using `git`.
- `dbar::tmux::TmuxProbe`: trait or helper that extracts session/pane/socket
  info from environment or `tmux display-message` output.
- `dbar::render`: renderer that applies `claude-status` palette and glyphs and
  emits tmux `#[fg=colourNN]`/`#[bg=colourNN]` styling.
- `dbar::install::TmuxInstaller`: helper that writes a tmux snippet using
  markers and returns a summary of changes.
- `dbar::cache`: helper that resolves the XDG cache path and manages cached
  lookup files.

Planned dependencies (subject to `cargo search` for versions):

- runtime: `ortho_config`, `serde`, `serde_json`, `camino`, `cap-std`,
  `thiserror`, `directories`, `mockable`, `clap`
- dev: `rstest`, `rstest-bdd`, `rstest-bdd-macros`, `mockall`, `assert_cmd`,
  `insta`, `tempfile`, `predicates`

## Revision note (required when editing an ExecPlan)

Initial draft created on 2026-01-05 to cover tmux status bar implementation,
configuration, tests, and documentation updates.

Revised 2026-01-05 to incorporate tmux status line protocol guidance, add the
`install` subcommand requirement, and plan for XDG cache usage via the
`directories` crate.

Revised 2026-01-05 to mark the plan as in progress after approval.

Revised 2026-01-05 to record scaffolding progress, cargo permission
discoveries, and the added `clap` dependency.

Revised 2026-01-05 to capture integration test discovery, clap defaults, and
progress updates.

Revised 2026-01-05 to mark the plan complete with final outcomes.

Revised 2026-01-05 to document the pane current path update.
