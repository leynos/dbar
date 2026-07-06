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
Installing with `--position right` also enables the clock by passing
`--show-clock true` to the status command.

Use `--dry-run` to preview the snippet without writing to disk:

```sh
cargo run -- install --path ~/.tmux.conf --dry-run
```

### Manual snippet

If you prefer to edit tmux manually, use a command substitution and pass tmux
formats into `dbar status`:

```tmux
set -g status-right '#(dbar status \
  --project-dir "#{pane_current_path}" \
  --session "#{session_name}" \
  --window "#{window_index}" \
  --pane "#{pane_id}" \
  --socket "#{socket_path}" \
  --show-clock true)'
```

Tmux supports line continuations with trailing backslashes, so this snippet can
be wrapped for readability.

To right-align the tmux segment when a client width is supplied, append
`--client-width "#{client_width}"` to the command.

The tmux status line protocol and style tags are explained in
`docs/tmux-statuslines-in-a-nutshell.md`.

## Configuration

Configuration uses `ortho_config`, so values can be supplied via configuration
files, environment variables, or CLI flags. The prefix is `DBAR`, and
subcommand settings live under `cmds.status` or `cmds.install` in the config
file. Environment variables use the `DBAR_CMDS_STATUS_` or `DBAR_CMDS_INSTALL_`
prefixes.

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

## Caching

dbar caches GitHub PR lookups under the XDG cache directory using the
`directories` crate. Override the cache directory with `--cache-dir` if needed,
and adjust the TTL with `--pr-cache-ttl-seconds`.
