# dbar user's guide

## Purpose

dbar renders a tmux-friendly status segment that shows the current project
name, git branch and status, upstream divergence, pull request number, worktree
indicator, tmux session details, and an optional clock. It outputs tmux
`#[...]` style tags so it can be embedded directly in the tmux status line.
For the change from the pre-0.2 usage to this CLI, see the [0.2 migration
guide](migration-0.2.md).

## Quick start

dbar runs on Unix only; it relies on POSIX process groups and `flock`, so it
will not build or run on Windows. tmux itself is a Unix tool, so this is not a
practical limitation for its intended use.

Build and run from a git repository:

```sh
cargo run -- status
```

Run with explicit tmux context (useful for tests or scripts):

```sh
cargo run -- status --session demo --window 1 --pane %0
```

## tmux integration

### Install helper

The `install` subcommand inserts an idempotent snippet into a tmux
configuration file. It writes a marked block so the snippet can be updated in
place on subsequent runs.

```sh
cargo run -- install --path ~/.tmux.conf --position left
```

The install command defaults to `--path ~/.tmux.conf --position left` when no
arguments are supplied.

Use `--full` to include the client width in the status command and add a
matching `status-left-length 999` (or `status-right-length 999`) directive.
Setting the length to 999 only raises tmux's own display-width cap; it does
not stop tmux from redrawing, and content can still be truncated on narrow
clients where the terminal width is the limiting factor.
Installing with `--position right` also enables the clock by passing
`--show-clock true` to the status command.

Use `--dry-run` to preview the snippet without writing to disk:

```sh
cargo run -- install --path ~/.tmux.conf --dry-run
```

An install reports whether it updated the file or found it already up to date.
When an existing file is changed, dbar writes a sibling backup before the
replacement. A dry run reports the generated snippet and does not create the
configuration file, its parent directory, a lock, or a backup. Existing dbar
markers are updated in place; incomplete or duplicate marker blocks are
reported as errors so unrelated configuration is not rewritten accidentally.

### Manual snippet

To edit tmux manually, use a command substitution and pass tmux formats into
`dbar status`:

```tmux
set -g status-right '#(dbar status \
  --project-dir #{q:pane_current_path} \
  --session #{q:session_name} \
  --window #{q:window_index} \
  --pane #{q:pane_id} \
  --socket #{q:socket_path} \
  --show-clock true)'
```

Tmux supports line continuations with trailing backslashes, so this snippet can
be wrapped for readability. The `#{q:...}` form makes tmux shell-quote each
value before it is spliced into the `#(...)` command, preventing shell
injection.

To right-align the tmux segment when a client width is supplied, append
`--client-width #{q:client_width}` to the command.

The tmux status line protocol and style tags are explained in
`docs/tmux-statuslines-in-a-nutshell.md`.

## Configuration

Configuration uses `ortho_config`, so values can be supplied via configuration
files, environment variables, or CLI flags. `.dbar.toml` defaults are
overridden by `DBAR_*` environment variables, which are in turn overridden by
CLI flags. The prefix is `DBAR`, and subcommand settings live under
`cmds.status` or `cmds.install` in the config file. Environment variables use
the `DBAR_CMDS_STATUS_` or `DBAR_CMDS_INSTALL_` prefixes.

Example `.dbar.toml`:

```toml
[cmds.status]
show_pr = false
show_clock = true
clock_format = "%H:%M"
pr_cache_ttl_seconds = 60
client_width = 120

[cmds.install]
position = "right"
dry_run = false
full = true
```

### Boolean settings

`install`'s `--dry-run` and `--full` are bare flags that take no value, yet
they follow the same precedence as every other setting. Absence of a flag is
distinct from `false`: an omitted flag contributes nothing to the merge, so a
configuration file or a `DBAR_*` variable decides, and a flag that is typed
wins over both.

```toml
[cmds.install]
dry_run = true
full = true
```

The same values can be supplied as `DBAR_CMDS_INSTALL_DRY_RUN=true` and
`DBAR_CMDS_INSTALL_FULL=true`, which override the file.

One asymmetry remains, and it is a property of the flag spelling rather than of
the merge: because `--dry-run` and `--full` accept no value, the command line
can turn a setting on but cannot turn one off. A configuration file that sets
`dry_run = true` is therefore overridden by setting
`DBAR_CMDS_INSTALL_DRY_RUN=false`, not from the command line.

### Invalid values

A malformed value in a configuration file or an environment variable is
reported as a merge failure naming the offending setting, and dbar exits
without rendering or installing anything. A malformed command-line argument is
reported by clap in the usual way. Neither case falls back to a default
silently.

## Diagnostics

dbar degrades silently by design: a failed `git`, `gh`, or `tmux` probe never
breaks the rendered status line, it just omits or simplifies the affected
segment. To inspect what was absorbed, set the `DBAR_DIAGNOSTICS`
environment variable to any value before running `dbar status`. Every probe
failure behind the rendered line is then printed to stderr, one per line.
Setting `DBAR_DIAGNOSTICS` is opt-in and never changes stdout: the printed
status line is identical whether or not the variable is set, so it is safe
to enable inside a tmux status command without affecting the segment tmux
displays.

## Status outcomes and defaults

`status` defaults to the current working directory, attempts to show the pull
request number, leaves the clock disabled, and uses `%H:%M` when the clock is
enabled. Supplying `--project-dir` points the git probes at another directory;
tmux session, window, pane, and socket values can be supplied explicitly or
resolved from the current tmux server.

Missing or failed git, GitHub, and tmux probes are fallbacks rather than fatal
status errors: dbar omits or simplifies the affected segment and still emits a
status line. An invalid clock format or invalid configuration value is a
configuration error and stops the command instead of silently choosing a
different value. Standard output remains reserved for the tmux status line;
diagnostic details are opt-in on standard error as described above.

## Caching

dbar caches GitHub PR lookups under the XDG cache directory using the
`directories` crate. Override the cache directory with `--cache-dir` if needed,
and adjust the TTL with `--pr-cache-ttl-seconds`. The default TTL is 60 seconds.

A fresh cache entry is used without invoking GitHub. A missing, expired,
unreadable, or malformed entry triggers a fresh lookup. A successful GitHub
number, an explicit no-PR result, or a `pr/<number>`/`pr-<number>` style branch
fallback is cached; a failed GitHub lookup is not cached, so a transient
failure does not persist for the whole TTL. If the cache directory cannot be
resolved or written, dbar still renders the value from the live lookup or
branch fallback and reports the degradation only when diagnostics are enabled.
