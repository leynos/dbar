//! Rendering logic for tmux status lines.

use crate::git::GitStatus;
use crate::tmux::TmuxContext;
use crate::types::{PrNumber, ProjectName};

const GLYPH_FADE_RIGHT: &str = "\u{e0c6}";
const GLYPH_BRANCH: &str = "\u{f418}";
const GLYPH_PR: &str = "\u{f408}";
const GLYPH_CHIP: &str = "\u{e266}";
const GLYPH_DIRTY: &str = "\u{f444}";
const GLYPH_STAGED: &str = "\u{f457}";
const GLYPH_AHEAD: &str = "\u{f432}";
const GLYPH_BEHIND: &str = "\u{f433}";
const GLYPH_CLEAN: &str = "\u{f42e}";

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
/// let line = render_status_line(&project, Some(&git), None, Some(&TmuxContext::default()));
/// assert!(line.contains("main"));
/// ```
pub fn render_status_line(
    project: &ProjectName,
    git_status: Option<&GitStatus>,
    pr_number: Option<&PrNumber>,
    tmux: Option<&TmuxContext>,
) -> String {
    let mut parts = Vec::new();

    parts.push(render_project_segment(project));

    if let Some(status) = git_status {
        parts.push(render_branch_segment(status));
        if status.is_worktree {
            parts.push(render_worktree_indicator());
        }
    }

    if let Some(pr) = pr_number {
        parts.push(render_pr_segment(pr));
    }

    if let Some(tmux_context) = tmux
        && let Some(segment) = render_tmux_segment(tmux_context)
    {
        parts.push(segment);
    }

    parts.join(" ")
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
        "{}{} wt{}",
        style(Some(COLOUR_CHIP_WARN), None),
        GLYPH_CHIP,
        reset()
    )
}

fn render_tmux_segment(context: &TmuxContext) -> Option<String> {
    let session = context.session.as_ref()?;
    let window = context.window.as_deref().unwrap_or("-");
    let pane = context.pane.as_deref().unwrap_or("-");
    let socket_hint = context.socket.as_ref().map(|value| socket_hint(value));

    let mut label = format!("{session}:{window}.{pane}");
    if let Some(socket) = socket_hint {
        label.push('@');
        label.push_str(&socket);
    }

    Some(format!(
        "{}{} {}{}",
        style(Some(COLOUR_PROJECT_FG), None),
        GLYPH_CHIP,
        label,
        reset()
    ))
}

fn socket_hint(socket: &str) -> String {
    socket.rsplit('/').next().unwrap_or(socket).to_owned()
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
        let line = render_status_line(&project, Some(&status), Some(&pr), Some(&tmux));
        assert!(line.contains("main"));
        assert!(line.contains("#17"));
    }
}
