//! Rendering logic for tmux status lines.

use crate::git::GitStatus;
use crate::tmux::TmuxContext;
use crate::types::{PrNumber, ProjectName};
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

/// Render a tmux status line from the collected probe data.
///
/// # Examples
///
/// ```rust,ignore
/// use dbar::git::GitStatus;
/// use dbar::render::RenderContext;
/// use dbar::render::render_status_line;
/// use dbar::tmux::TmuxContext;
/// use dbar::types::{BranchName, ProjectName};
/// use dbar::types::{AheadCount, BehindCount};
///
/// let project = ProjectName::new("demo");
/// let git = GitStatus {
///     branch: BranchName::new("main"),
///     dirty: false,
///     staged: false,
///     ahead: AheadCount::new(0),
///     behind: BehindCount::new(0),
///     is_worktree: false,
/// };
/// let context = RenderContext {
///     project: &project,
///     git_status: Some(&git),
///     pr_number: None,
///     tmux: Some(&TmuxContext::default()),
///     clock: None,
///     client_width: None,
/// };
/// let line = render_status_line(&context);
/// assert!(line.contains("main"));
/// ```
pub struct RenderContext<'a> {
    /// Project name rendered in the status line.
    pub project: &'a ProjectName,
    /// Optional git status data.
    pub git_status: Option<&'a GitStatus>,
    /// Optional PR number to render.
    pub pr_number: Option<&'a PrNumber>,
    /// Optional tmux metadata for the right segment.
    pub tmux: Option<&'a TmuxContext>,
    /// Optional clock label for the final right segment.
    pub clock: Option<&'a str>,
    /// Optional tmux client width used for right alignment.
    pub client_width: Option<usize>,
}

/// Render a tmux status line from the collected probe data.
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
    let segment = format!(
        "{} {} {}{}{}{}",
        style(Some(COLOUR_PROJECT_FG), Some(COLOUR_PROJECT_BG)),
        project,
        reset_bg_with_fg(COLOUR_PROJECT_BG),
        GLYPH_FADE_RIGHT,
        style(None, None),
        reset(),
    );
    segment
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
        status.branch
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
        "{}{} #{}{}",
        style(Some(COLOUR_PR), None),
        GLYPH_PR,
        pr,
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

fn render_tmux_segment(context: &TmuxContext) -> Option<String> {
    let session = context.session.as_ref()?;
    let window = context.window.as_deref().unwrap_or("-");
    let pane = context.pane.as_deref().unwrap_or("-");

    let label = format!("{session}:{window}.{pane}");

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
        clock,
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

fn visible_width(value: &str) -> usize {
    let mut width = 0;
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
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
mod tests {
    //! Tests for status-line rendering, style tags, and glyph emission.
    use super::*;
    use crate::types::{AheadCount, BehindCount, BranchName, PrNumber, ProjectName};

    #[test]
    fn render_includes_branch_and_pr() {
        let project = ProjectName::new("demo");
        let status = GitStatus {
            branch: BranchName::new("main"),
            dirty: false,
            staged: false,
            ahead: AheadCount::new(0),
            behind: BehindCount::new(0),
            is_worktree: false,
        };
        let tmux = TmuxContext {
            session: Some("session".into()),
            window: Some("1".into()),
            pane: Some("%0".into()),
            socket: None,
        };
        let pr = PrNumber::new("17");
        let context = RenderContext {
            project: &project,
            git_status: Some(&status),
            pr_number: Some(&pr),
            tmux: Some(&tmux),
            clock: None,
            client_width: None,
        };
        let line = render_status_line(&context);
        assert!(line.contains("main"));
        assert!(line.contains("#17"));
    }

    #[test]
    fn layout_right_justifies_with_width() {
        let output = layout_with_width("left", "right", 12);
        assert_eq!(output, "left   right");
    }

    #[test]
    fn render_places_clock_after_tmux_on_right() {
        let project = ProjectName::new("demo");
        let tmux = TmuxContext {
            session: Some("session".into()),
            window: Some("1".into()),
            pane: Some("%0".into()),
            socket: None,
        };
        let context = RenderContext {
            project: &project,
            git_status: None,
            pr_number: None,
            tmux: Some(&tmux),
            clock: Some("09:41"),
            client_width: None,
        };
        let line = render_status_line(&context);
        assert!(line.contains("session:1.%0"));
        assert!(line.ends_with(" 09:41#[default]"));
    }
}
