# dbar

[![Ask DeepWiki](https://deepwiki.com/badge.svg)](
https://deepwiki.com/leynos/dbar)

`dbar` renders a tmux status line segment that mirrors the aesthetics and
glyphs from `~/.local/bin/claude-status`, while also adding GitHub PR
information, worktree indicators, and tmux context. It is designed to be driven
by tmux formats and configured with `ortho_config`.

## Features

- Project name derived from git remotes or directory names.
- Git branch, dirty/staged indicators, and upstream ahead/behind counts.
- GitHub PR number rendering with caching.
- Worktree indicator and tmux session info.
- `install` subcommand that inserts an idempotent tmux snippet.
- Optional clock rendering in the right-hand segment.
- Client-width-aware layout to right-align the tmux and clock segment.

## Quick start

Render a status line directly:

```sh
cargo run -- status --project-dir . --show-pr false --session demo --window 1 --pane %0
```

Install the snippet into tmux config (defaults to `~/.tmux.conf` and
`status-left`):

```sh
cargo run -- install
```

Reload tmux:

```sh
tmux source-file ~/.tmux.conf
```

## tmux integration

`dbar` expects tmux to pass pane and session metadata. The `install` subcommand
inserts a snippet like:

```tmux
set -g status-left '#(dbar status \
  --project-dir "#{pane_current_path}" \
  --session "#{session_name}" \
  --window "#{window_index}" \
  --pane "#{pane_id}" \
  --socket "#{socket_path}")'
```

To enable client-width-aware right alignment, either run:

```sh
cargo run -- install --full
```

or append `--client-width "#{client_width}"` to the manual snippet.

Installing to `status-right` enables the clock automatically:

```sh
cargo run -- install --position right
```

## Configuration

Configuration uses `ortho_config`. Defaults can be supplied in `.dbar.toml`,
environment variables (`DBAR_*`), or CLI flags. Subcommand config lives under
`cmds.status` and `cmds.install`:

```toml
[cmds.status]
show_pr = false
show_clock = true
clock_format = "%H:%M"
pr_cache_ttl_seconds = 60
client_width = 120

[cmds.install]
position = "left"
full = true
```

## Development

Run formatters and checks:

```sh
make fmt
make check-fmt
make lint
make test
```

## Further documentation

See `docs/users-guide.md` and `docs/tmux-statuslines-in-a-nutshell.md` for more
detail on tmux status line behaviour and configuration options.
