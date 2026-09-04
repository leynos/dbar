//! Building the managed tmux snippet and splicing it into a config.
//!
//! Kept apart from the filesystem layer in [`super::fs`] so the decision of
//! *what* the config should contain is testable without touching a disk.

use crate::types::StatusPosition;

use super::{InstallError, Width};

pub(super) const MARKER_START: &str = "# dbar: begin";
pub(super) const MARKER_END: &str = "# dbar: end";

/// Decide what the config should hold once `snippet` is installed.
///
/// The marker pair is counted before anything is rewritten. Splitting on the
/// *first* marker alone cannot tell one managed block from several, so a config
/// carrying duplicates would otherwise be rewritten around its first block and
/// leave the rest behind — or, when the first block already matched, be
/// reported as up to date while a second block still fought over the same
/// option. Neither outcome is repairable by re-running the install, so a
/// duplicated block is refused and left for the user to resolve.
pub(super) fn apply_snippet(existing: &str, snippet: &str) -> Result<(bool, String), InstallError> {
    match (
        existing.matches(MARKER_START).count(),
        existing.matches(MARKER_END).count(),
    ) {
        (0, 0) => Ok(append_snippet(existing, snippet)),
        (1, 1) => replace_block(existing, snippet),
        (starts, ends) if starts > 1 || ends > 1 => Err(InstallError::DuplicateMarkers),
        _ => Err(InstallError::IncompleteMarkers),
    }
}

/// Rewrite the config's single managed block, reporting whether it changed.
fn replace_block(existing: &str, snippet: &str) -> Result<(bool, String), InstallError> {
    // The markers are known to appear once each, but the end marker may still
    // precede the start marker, which leaves no well-formed block to replace.
    let Some((before, after_start)) = existing.split_once(MARKER_START) else {
        return Err(InstallError::IncompleteMarkers);
    };
    let Some((between, after_marker_end)) = after_start.split_once(MARKER_END) else {
        return Err(InstallError::IncompleteMarkers);
    };
    let (line_break, after_end) = after_marker_end
        .strip_prefix('\n')
        .map_or(("", after_marker_end), |rest| ("\n", rest));
    let current = format!("{MARKER_START}{between}{MARKER_END}{line_break}");
    if current == snippet {
        return Ok((false, existing.to_owned()));
    }
    let mut next = String::new();
    next.push_str(before);
    next.push_str(snippet);
    next.push_str(after_end);
    Ok((true, next))
}

/// Append the managed block to a config that carries no markers at all.
fn append_snippet(existing: &str, snippet: &str) -> (bool, String) {
    let mut next = String::from(existing);
    if !next.ends_with('\n') && !next.is_empty() {
        next.push('\n');
    }
    next.push_str(snippet);
    (true, next)
}

/// The one class of characters tmux's `#{q:...}` modifier leaves unescaped.
///
/// `q:` backslash-escapes every shell metacharacter, along with space, `=` and
/// `%` — but not the control characters. A newline is therefore passed through
/// bare, and `/bin/sh` reads it as a command terminator, so a directory whose
/// name embeds a newline splits the status command in two and runs the second
/// half. Directory names may contain any byte but `/` and NUL, so this is
/// reachable rather than theoretical. Verified against tmux next-3.4, where the
/// injection fires without this guard and does not fire with it.
///
/// The class is written with literal control bytes because tmux's format parser
/// treats `:` as the modifier terminator, which makes `[[:cntrl:]]` unusable,
/// and `[^ -~]` would strip every non-ASCII byte from legitimate paths.
const TMUX_CONTROL_CHARACTERS: &str = "[\u{1}-\u{1f}\u{7f}]";

/// Interpolate a tmux format into the `#(...)` command as one literal argument.
///
/// tmux applies the modifiers innermost first, so the value is shell-escaped by
/// `q:` and the control characters `q:` ignores are only then replaced. The
/// result is spliced in *unquoted*: `q:` escapes rather than quotes, so
/// wrapping the slot in quotes would break the escaping instead of reinforcing
/// it. A path carrying a control character is mangled rather than honoured,
/// which costs a wrong status line in a case no shell could have handled
/// safely anyway.
pub(super) fn quoted_format(format: &str) -> String {
    format!("#{{s/{TMUX_CONTROL_CHARACTERS}/_/:#{{q:{format}}}}}")
}

/// Build the managed block that binds `dbar status` to a tmux status option.
///
/// The block sets `status-left` or `status-right` (per `position`) to a
/// `#(...)` command of the form:
///
/// ```text
/// dbar status --project-dir <pane_current_path> --session <session_name> \
///     --window <window_index> --pane <pane_id> --socket <socket_path> \
///     [--show-clock true] [--client-width <client_width>]
/// ```
///
/// Every value is a tmux format interpolated through [`quoted_format`], so the
/// arguments are resolved by tmux at render time rather than frozen at install
/// time. The five leading flags are unconditional and always appear in that
/// order; the CLI parses them by name, so the order is a readability contract
/// rather than a positional one.
///
/// `--show-clock true` is emitted only for [`StatusPosition::Right`]. The clock
/// is right-aligned against the end of the status line, which only the
/// right-hand segment owns; adding it on the left would plant a clock in the
/// middle of the bar, so the left variant leaves the flag off and takes the
/// CLI's default.
///
/// `--client-width` is emitted only for [`Width::Full`], the variant that also
/// raises `{target}-length` to 999 so the segment may claim the whole bar. Only
/// then does the renderer need the client's width to know how much room it has;
/// the plain variant is bounded by tmux's own length cap instead, and passing a
/// width there would invite it to render past that limit and be truncated.
pub(super) fn build_snippet(position: StatusPosition, width: Width) -> String {
    let target = match position {
        StatusPosition::Left => "status-left",
        StatusPosition::Right => "status-right",
    };
    let mut command = format!(
        "dbar status --project-dir {} --session {} --window {} --pane {} --socket {}",
        quoted_format("pane_current_path"),
        quoted_format("session_name"),
        quoted_format("window_index"),
        quoted_format("pane_id"),
        quoted_format("socket_path"),
    );
    if matches!(position, StatusPosition::Right) {
        command.push_str(" --show-clock true");
    }
    if width.is_full() {
        command.push_str(" --client-width ");
        command.push_str(&quoted_format("client_width"));
    }
    let length_line = if width.is_full() {
        format!("set -g {target}-length 999\n")
    } else {
        String::new()
    };
    format!("{MARKER_START}\nset -g {target} '#({command})'\n{length_line}{MARKER_END}\n")
}
