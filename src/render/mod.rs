//! Rendering logic for tmux status lines.

use crate::git::GitStatus;
use crate::types::{BranchName, PrNumber, ProjectName};
use unicode_width::UnicodeWidthChar;

const GLYPH_FADE_RIGHT: &str = "\u{e0c6}";
const GLYPH_BRANCH: &str = "\u{f418}";
const GLYPH_PR: &str = "\u{f408}";
const GLYPH_WORKTREE: &str = "\u{f0e69}";
const GLYPH_DIRTY: &str = "\u{f444}";
const GLYPH_STAGED: &str = "\u{f457}";
const GLYPH_AHEAD: &str = "\u{f432}";
const GLYPH_BEHIND: &str = "\u{f433}";
const GLYPH_CLEAN: &str = "\u{f42e}";
const GLYPH_TMUX: &str = "\u{ebc8}";
const GLYPH_CLOCK: &str = "\u{f017}";

const COLOUR_PROJECT_BG: u8 = 24;
const COLOUR_PROJECT_FG: u8 = 117;
const COLOUR_BRANCH_CLEAN: u8 = 114;
const COLOUR_BRANCH_DIRTY: u8 = 221;
const COLOUR_PR: u8 = 176;
const COLOUR_CHIP_WARN: u8 = 221;
const COLOUR_CHIP_DANGER: u8 = 203;

/// Label drawn when git reports no current branch.
///
/// The substitution belongs here rather than in the probes: it is a rendering
/// decision, and minting it earlier would make a detached `HEAD` and a real
/// branch called `detached` indistinguishable to the PR lookup and the cache
/// key.
const DETACHED_LABEL: &str = "detached";

/// Tmux values normalized for presentation.
///
/// The renderer owns this small snapshot so rendering does not depend on the
/// probe adapter that collected it.
#[derive(Debug, Clone, Default)]
pub struct RenderTmuxContext {
    /// Tmux session name.
    pub session: Option<String>,
    /// Tmux window index or name.
    pub window: Option<String>,
    /// Tmux pane identifier.
    pub pane: Option<String>,
    /// Tmux server socket path.
    pub socket: Option<String>,
}

/// Borrowed inputs for one status-line render.
///
/// Every field is a snapshot already gathered by the probes: the project name,
/// the optional git and pull-request data for the left segment, the optional
/// tmux metadata and clock for the right segment, and the optional client width
/// used to right-align that segment.
pub struct RenderContext<'a> {
    /// Project name rendered in the status line.
    pub project: &'a ProjectName,
    /// Optional git status data.
    pub git_status: Option<&'a GitStatus>,
    /// Optional PR number to render.
    pub pr_number: Option<&'a PrNumber>,
    /// Optional tmux metadata for the right segment.
    pub tmux: Option<&'a RenderTmuxContext>,
    /// Optional clock label for the final right segment.
    pub clock: Option<&'a str>,
    /// Optional tmux client width used for right alignment.
    pub client_width: Option<usize>,
}

/// Render a tmux status line from the collected probe data.
///
/// The left segment carries the project name, branch and pull-request chips;
/// the right segment carries the tmux location and clock. Dynamic values are
/// escaped so tmux renders them literally, while the renderer's own `#[...]`
/// style tags are emitted unescaped.
///
/// # Examples
///
/// ```text
/// let project = ProjectName::new("demo");
/// let git = GitStatus {
///     branch: Some(BranchName::new("main")),
///     dirty: false,
///     staged: false,
///     ahead: AheadCount::new(0),
///     behind: BehindCount::new(0),
///     is_worktree: false,
/// };
/// let tmux = RenderTmuxContext {
///     session: Some("work".to_owned()),
///     window: Some("1".to_owned()),
///     pane: Some("%0".to_owned()),
///     socket: Some("/tmp/tmux-1000/build".to_owned()),
/// };
/// let context = RenderContext {
///     project: &project,
///     git_status: Some(&git),
///     pr_number: None,
///     tmux: Some(&tmux),
///     clock: Some("09:41"),
///     client_width: None,
/// };
/// let line = render_status_line(&context);
///
/// // The project and branch open the line, the tmux location names the
/// // non-default socket, and the clock closes it.
/// assert!(line.contains("demo"));
/// assert!(line.contains("main"));
/// assert!(line.contains("work:1.%0@build"));
/// assert!(line.ends_with(" 09:41#[default]"));
/// ```
pub fn render_status_line(context: &RenderContext<'_>) -> String {
    let mut parts = Vec::new();

    parts.push(render_project_segment(context.project));

    if let Some(status) = context.git_status {
        parts.push(render_branch_segment(status));
        if status.is_worktree {
            parts.push(render_worktree_indicator());
        }
    }

    if let Some(pr) = context.pr_number {
        parts.push(render_pr_segment(pr));
    }

    let left = parts.join(" ");

    let right = render_right_segment(context);
    match (context.client_width, right) {
        (Some(width), Some(segment)) => layout_with_width(&left, &segment, width),
        (None, Some(segment)) => format!("{left} {segment}"),
        (_, None) => left,
    }
}

fn render_right_segment(context: &RenderContext<'_>) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(segment) = context.tmux.and_then(render_tmux_segment) {
        parts.push(segment);
    }
    if let Some(clock) = context.clock {
        parts.push(render_clock_segment(clock));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

fn render_project_segment(project: &ProjectName) -> String {
    format!(
        "{} {} {}{}{}{}",
        style(Some(COLOUR_PROJECT_FG), Some(COLOUR_PROJECT_BG)),
        escape_tmux(project.as_ref()),
        reset_bg_with_fg(COLOUR_PROJECT_BG),
        GLYPH_FADE_RIGHT,
        style(None, None),
        reset(),
    )
}

/// The branch text to draw, standing in for an absent branch.
fn branch_label(status: &GitStatus) -> &str {
    status
        .branch
        .as_ref()
        .map_or(DETACHED_LABEL, BranchName::as_ref)
}

fn render_branch_segment(status: &GitStatus) -> String {
    let branch_colour = if status.dirty {
        COLOUR_BRANCH_DIRTY
    } else {
        COLOUR_BRANCH_CLEAN
    };
    let mut segment = vec![format!(
        "{}{} {}",
        style(Some(branch_colour), None),
        GLYPH_BRANCH,
        escape_tmux(branch_label(status))
    )];

    let mut indicators = Vec::new();
    if status.staged {
        indicators.push(format!(
            "{}{}",
            style(Some(COLOUR_BRANCH_CLEAN), None),
            GLYPH_STAGED
        ));
    }
    if status.dirty {
        indicators.push(format!(
            "{}{}",
            style(Some(COLOUR_BRANCH_DIRTY), None),
            GLYPH_DIRTY
        ));
    }
    if status.ahead.value() > 0 {
        indicators.push(format!(
            "{}{}{}",
            style(Some(COLOUR_PROJECT_FG), None),
            GLYPH_AHEAD,
            status.ahead
        ));
    }
    if status.behind.value() > 0 {
        indicators.push(format!(
            "{}{}{}",
            style(Some(COLOUR_CHIP_DANGER), None),
            GLYPH_BEHIND,
            status.behind
        ));
    }
    if indicators.is_empty() && !status.dirty {
        indicators.push(format!(
            "{}{}",
            style(Some(COLOUR_BRANCH_CLEAN), None),
            GLYPH_CLEAN
        ));
    }

    if !indicators.is_empty() {
        segment.push(indicators.join(" "));
    }

    segment.push(reset().to_owned());
    segment.join(" ")
}

fn render_pr_segment(pr: &PrNumber) -> String {
    format!(
        // The literal `#` prefix is escaped too: an unescaped `#` immediately
        // followed by a hostile PR value starting with `{` would otherwise
        // form a `#{...}` format sequence.
        "{}{} ##{}{}",
        style(Some(COLOUR_PR), None),
        GLYPH_PR,
        escape_tmux(&pr.to_string()),
        reset()
    )
}

fn render_worktree_indicator() -> String {
    format!(
        "{}{}{}",
        style(Some(COLOUR_CHIP_WARN), None),
        GLYPH_WORKTREE,
        reset()
    )
}

/// Name of the socket tmux creates when no `-L`/`-S` override is given.
const DEFAULT_SOCKET_NAME: &str = "default";

/// Abbreviate a tmux socket path to the server name it identifies.
///
/// The status line is width-constrained, so the full path is never rendered:
/// only the final path component is, and only when it names a server other
/// than tmux's default one. A plain `tmux` session therefore renders exactly
/// as before, while `tmux -L build` is distinguishable at a glance.
fn socket_label(socket: &str) -> Option<&str> {
    socket
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty() && *name != DEFAULT_SOCKET_NAME)
}

fn render_tmux_segment(context: &RenderTmuxContext) -> Option<String> {
    let session = context.session.as_ref()?;
    let window = context.window.as_deref().unwrap_or("-");
    let pane = context.pane.as_deref().unwrap_or("-");

    let mut location = format!("{session}:{window}.{pane}");
    if let Some(socket) = context.socket.as_deref().and_then(socket_label) {
        location.push('@');
        location.push_str(socket);
    }
    let label = escape_tmux(&location);

    Some(format!(
        "{}{} {}{}",
        style(Some(COLOUR_PROJECT_FG), None),
        GLYPH_TMUX,
        label,
        reset()
    ))
}

fn render_clock_segment(clock: &str) -> String {
    format!(
        "{}{} {}{}",
        style(Some(COLOUR_PROJECT_FG), None),
        GLYPH_CLOCK,
        escape_tmux(clock),
        reset()
    )
}

fn layout_with_width(left: &str, right: &str, width: usize) -> String {
    let left_len = visible_width(left);
    let right_len = visible_width(right);
    if width <= left_len + right_len + 1 {
        return format!("{left} {right}");
    }

    let pad = width - left_len - right_len;
    let mut output = String::with_capacity(left.len() + right.len() + pad);
    output.push_str(left);
    output.extend(std::iter::repeat_n(' ', pad));
    output.push_str(right);
    output
}

/// Escape a dynamic value so tmux renders it literally.
///
/// tmux treats `#` as the introducer for styles (`#[...]`), formats (`#{...}`)
/// and commands (`#(...)`) in the output it substitutes into the status line.
/// Doubling `#` is tmux's documented literal-`#` escape, so a hostile branch
/// name or path cannot inject markup into the rendered segment.
fn escape_tmux(value: &str) -> String {
    value.replace('#', "##")
}

fn visible_width(value: &str) -> usize {
    let mut width = 0;
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        // `##` is an escaped literal `#`, occupying a single column.
        if ch == '#' && matches!(chars.peek(), Some('#')) {
            chars.next();
            width += 1;
            continue;
        }
        if ch == '#' && matches!(chars.peek(), Some('[')) {
            skip_style(&mut chars);
            continue;
        }
        width += UnicodeWidthChar::width(ch).unwrap_or(0);
    }
    width
}

fn skip_style(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    chars.next();
    for next in chars.by_ref() {
        if next == ']' {
            break;
        }
    }
}

fn style(fg: Option<u8>, bg: Option<u8>) -> String {
    match (fg, bg) {
        (Some(foreground), Some(background)) => {
            format!("#[fg=colour{foreground},bg=colour{background}]")
        }
        (Some(foreground), None) => format!("#[fg=colour{foreground}]"),
        (None, Some(background)) => format!("#[bg=colour{background}]"),
        (None, None) => "#[default]".to_owned(),
    }
}

fn reset_bg_with_fg(colour: u8) -> String {
    format!("#[fg=colour{colour},bg=default]")
}

const fn reset() -> &'static str {
    "#[default]"
}

#[cfg(test)]
mod property_tests;
#[cfg(test)]
mod tests;
