# tmux status line “protocol” (Powerline-style segments)

tmux doesn’t have a separate, structured “status line protocol” like i3bar’s
JSON. Instead, it gives you three composable building blocks:

1. **A format language** for dynamic values: `#{…}`
2. **Inline style tags** for colours and attributes: `#[…]`
3. **Command substitution** to splice in external output: `#( … )`

Powerline (and similar setups) work by generating text that includes `#[…]`
style tags plus Unicode separator glyphs.

______________________________________________________________________

## The mental model

Think of the status line as a string that tmux *renders*.

- tmux expands **formats** like `#{session_name}`.
- tmux applies **styles** like `#[fg=…,bg=…,bold]`.
- tmux runs **commands** like `#(~/bin/status)` and inserts their output.

The “protocol” for your external command is simply: **print the status text you
want tmux to display**, optionally including tmux style tags.

______________________________________________________________________

## 1) `#( … )`: external commands

### What your command should output

- Output plain text.
- If you want colours/attributes, embed tmux style tags: `#[…]`.
- Keep it **short** and **fast**.

A tiny example script:

```sh
#!/bin/sh
printf '#[fg=colour235,bg=colour39,bold] OK #[default]'
```

Used from tmux:

```tmux
set -g status-right '#(~/bin/tmux-ok)'
```

### “Which line gets used?”

Treat it as “tmux inserts the last line your command prints”. In practice:
print one line and you’ll never have to think about it.

### What tmux passes to the command

tmux runs the command via `/bin/sh`.

Important nuance: it **does not run inside your pane**. That means you
shouldn’t assume you’ll get pane-scoped environment variables like `TMUX_PANE`.

If you need context (session/window/pane/client), you have two reliable
patterns:

#### A) Pass the context in as arguments using tmux formats

```tmux
set -g status-right '#(~/bin/myseg "#{session_name}" "#{window_index}" "#{pane_id}")'
```

Your script can then use `$1`, `$2`, `$3`.

#### B) Query tmux from inside the script

```sh
#!/bin/sh
sess=$(tmux display-message -p '#{session_name}')
win=$(tmux display-message -p '#{window_index}')
printf '%s:%s' "$sess" "$win"
```

Pattern A usually keeps things simpler and cheaper.

### Update cadence and performance

tmux doesn’t want your status scripts to be an accidental cryptocurrency miner.

Practical rules:

- Keep `#(…)` scripts fast (milliseconds, not seconds).
- Cache anything expensive yourself.
- Prefer a “daemon writes cache file; status reads cache file” arrangement if
  you’re doing network calls.

You can control normal polling with:

```tmux
set -g status-interval 5
```

If you want immediate refresh (e.g. after a hook), use:

```sh
tmux refresh-client -S
```

______________________________________________________________________

## 2) `#[…]`: colours and attributes

### Basic syntax

A style tag looks like:

- `#[fg=<colour>,bg=<colour>,<attributes>]`
- Reset with `#[default]`

Example:

```tmux
set -g status-left '#[fg=black,bg=yellow,bold] #S #[default]'
```

### Colours you can use

Common options:

- Named colours: `red`, `green`, `yellow`, `blue`, etc.
- 256-colour palette: `colour0` … `colour255`
- True-colour RGB: `#RRGGBB` (e.g. `#ff8800`)
- `default` to use tmux’s default

Examples:

```text
#[fg=colour235,bg=colour39]
#[fg=#0b1020,bg=#e0b000,bold]
#[fg=default,bg=default]
```

### Attributes

Common ones:

- `bold`
- `dim`
- `underscore`
- `italics`
- `reverse`
- `strikethrough`

You can combine them:

```text
#[fg=white,bg=colour52,bold,underscore]
```

Reset attributes using `#[default]` (easy) or `#[none]` (if you want to keep
colours but clear attributes).

______________________________________________________________________

## 3) `#{…}`: tmux formats (dynamic values)

Formats let you reference tmux state in strings.

Examples:

- `#{session_name}`
- `#{window_name}`
- `#{window_index}`
- `#{pane_id}`
- `#{pane_current_path}`

There are also conditionals:

```text
#{?client_prefix,#[bg=red] PREFIX #[default],}
```

That reads as: “if `client_prefix` is true, show a red PREFIX label; otherwise
show nothing”.

Formats are useful both directly in `status-left/right` and as arguments to
your `#(…)` commands.

______________________________________________________________________

## The Powerline trick: separators + colour transitions

Powerline’s look comes from two ideas:

1. **Segments** with a background colour
2. **A separator glyph** whose foreground matches the next segment’s background

A typical separator glyph is `` (requires a nerd-font/powerline-capable font).

Here’s a minimal “two segment” pattern:

```text
#[fg=colour235,bg=colour39] SEG1 #[fg=colour39,bg=colour234]#[fg=colour234,bg=colour220] SEG2 #[default]
```

Read it like a painter:

- Draw SEG1 on bg=39
- Draw the separator with fg=39 (so the triangle is SEG1’s background)
- Switch background to 234 behind the separator (so the triangle fades into the
  next bg)
- Draw SEG2

______________________________________________________________________

## A complete, friendly example

### ~/.tmux.conf

```tmux
set -g status on
set -g status-interval 5

# Session name on the left
set -g status-left '#[fg=colour231,bg=colour25,bold] #S #[default]'

# A Powerline-ish right side built from an external script
set -g status-right '#(~/bin/tmux-segs "#{session_name}" "#{window_index}" "#{pane_id}")'
```

### ~/bin/tmux-segs

```sh
#!/bin/sh
set -eu

session="$1"
win="$2"
pane="$3"

SEP=""  # needs a nerd/powerline font

# Segment A colours
A_FG="colour231"
A_BG="colour31"

# Segment B colours
B_FG="colour235"
B_BG="colour220"

# A then separator into B, then reset
printf '#[fg=%s,bg=%s,bold] %s ' "$A_FG" "$A_BG" "$session"
printf '#[fg=%s,bg=colour234]%s' "$A_BG" "$SEP"
printf '#[fg=%s,bg=%s] %s:%s ' "$B_FG" "$B_BG" "$win" "$pane"
printf '#[default]'
```

Make it executable:

```sh
chmod +x ~/bin/tmux-segs
```

______________________________________________________________________

## Hooks: refreshing when tmux state changes

Polling every N seconds is fine for clocks, but you may want immediate refresh
when tmux state changes.

tmux supports “hooks” that run commands on events. A common pattern is:

- hook runs your update action (write a cache file, etc.)
- hook runs `tmux refresh-client -S`

If you’re doing heavy work, don’t do it directly in the hook; have the hook
nudge your daemon.

______________________________________________________________________

## Common gotchas

### Fonts

Powerline separators need glyph support. Install a Nerd Font (or the older
Powerline-patched fonts) and configure your terminal to use it.

### Quoting

If you pass `#{…}` formats into `#(…)` as arguments, quote them.

Bad:

```tmux
set -g status-right '#(~/bin/seg #{session_name})'
```

Better:

```tmux
set -g status-right '#(~/bin/seg "#{session_name}")'
```

### `%` and time formatting

Some status strings can be interpreted by `strftime` as well as tmux’s format
expander (depending on the option). If you see mysterious behaviour with `%`,
escape it as `%%`.

### Don’t emit ANSI escape sequences

Use `#[…]` tags rather than raw `\x1b[…m` sequences.

### Keep scripts fast

If your status command sometimes hangs (DNS, a slow API, a laptop on Wi‑Fi in a
hotel made of concrete), your status line will feel cursed.

Cache and degrade gracefully.

______________________________________________________________________

## A good “architecture” for serious setups

If you want a polished, low-latency, low-jitter status line:

- A background process updates a cache (file or shared memory) on events/timers.
- The status line runs tiny commands that just print cached content.
- Hooks trigger `refresh-client -S` to redraw immediately.

That’s the grown-up version of “Powerline, but I don’t want it to stutter.”
