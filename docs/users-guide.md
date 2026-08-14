# dbar user's guide

## Purpose

dbar renders a tmux-friendly status segment that shows the current project
name, git branch and status, upstream divergence, pull request number, worktree
indicator, tmux session details, and an optional clock. It outputs tmux
`#[...]` style tags so it can be embedded directly in the tmux status line.

## Quick start

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
```

### Settings a configuration file cannot control

Two groups of settings do not follow the precedence above, because clap
supplies a value even when the corresponding flag is omitted, and that value
is indistinguishable from one a user typed on the command line:

- `install`'s `--dry-run` and `--full` flags are plain boolean flags. Clap
  materializes `false` for either one whenever it is absent, so the
  command-line layer always shadows a configuration file's `dry_run` or
  `full` setting. Declaring these as optional flags instead would make clap
  demand a value (`--full <VALUE>`), breaking the existing flag spelling, so
  this limitation is deliberate rather than an oversight.
- `status`'s `clock_format` and `pr_cache_ttl_seconds` declare a clap
  default, so clap likewise materializes that default whenever the flag is
  absent, and a configuration file's value for either setting is shadowed the
  same way. Only settings whose defaults come solely from the configuration
  layer (not from a clap default) can be overridden by a configuration file.

Any of these five settings can still be controlled from a configuration file
indirectly by pairing it with the matching `DBAR_*` environment variable
(for example `DBAR_CMDS_INSTALL_FULL=true`), since environment variables are
read by the same shell session and are not subject to this limitation.

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

## Caching

dbar caches GitHub PR lookups under the XDG cache directory using the
`directories` crate. Override the cache directory with `--cache-dir` if needed,
and adjust the TTL with `--pr-cache-ttl-seconds`.
