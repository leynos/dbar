# Migrating to dbar 0.2

dbar 0.2 introduces the command-line interface for rendering a tmux status
segment and installing its tmux configuration. The two entry points are
`dbar status` and `dbar install`.

## Update the tmux integration

Use the installer to add or update the managed block in the tmux configuration:

```sh
cargo run -- install
```

The default target is `~/.tmux.conf`, and the default position is
`status-left`. Use `--path` for another configuration file, `--position right`
for the right-hand segment, and `--dry-run` to inspect the generated block
before writing it. The right-hand position enables the clock. Add `--full` when
the segment should receive the tmux client width and raise the corresponding
status-length limit for right-aligned output.

The installer marks the block so later runs update it in place. A run that
changes an existing configuration writes a sibling backup; a run whose managed
block is already current reports no update. Incomplete or duplicate dbar marker
blocks must be repaired manually before installation can continue.

## Use the status command

Render a segment directly, or use the same command in a tmux `#(...)` status
format:

```sh
cargo run -- status
cargo run -- status --project-dir . --session demo --window 1 --pane %0
```

Status rendering defaults to the current directory, attempts a GitHub pull
request lookup, and leaves the clock disabled. Git, GitHub, and tmux failures
degrade the corresponding segment rather than failing the status command.
Standard output contains only the tmux status line. Set `DBAR_DIAGNOSTICS` to
mirror absorbed probe failures to standard error when troubleshooting.

## Configuration and cache changes

The status and install settings use `ortho_config`: configuration files,
`DBAR_*` environment variables, and command-line flags are merged in that
order of increasing precedence. Status settings live under `[cmds.status]`,
and install settings under `[cmds.install]`.

GitHub pull-request results are cached below the XDG cache directory for 60
seconds by default. A fresh entry avoids a GitHub call. Missing, expired, or
unreadable entries trigger a fresh lookup; successful results, including a
confirmed absence of a pull request, can be cached. A failed lookup is not
cached, so a transient GitHub failure does not become a stale result. Use
`--cache-dir` and `--pr-cache-ttl-seconds` to override the cache location and
expiry.

For the complete command reference and examples, see the
[user's guide](users-guide.md).
