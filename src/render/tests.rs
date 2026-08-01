//! Tests for status-line rendering, style tags, and glyph emission.
use super::*;
use crate::tmux::TmuxContext;
use crate::types::{AheadCount, BehindCount, BranchName, PrNumber, ProjectName};
use rstest::rstest;

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

#[test]
fn escape_tmux_doubles_every_hash() {
    assert_eq!(escape_tmux("plain"), "plain");
    assert_eq!(escape_tmux("#[fg=red]"), "##[fg=red]");
    assert_eq!(escape_tmux("#{pane_id}"), "##{pane_id}");
    assert_eq!(escape_tmux("#(id)"), "##(id)");
    assert_eq!(escape_tmux("a#b#c"), "a##b##c");
}

#[test]
fn visible_width_counts_escaped_hash_as_one_column() {
    assert_eq!(visible_width("##"), 1);
    assert_eq!(visible_width("a##b"), 3);
    // Renderer style tags remain zero-width.
    assert_eq!(visible_width("#[fg=colour1]ab#[default]"), 2);
}

/// Hostile values must not introduce unescaped tmux markup.
#[rstest]
#[case::style("#[fg=red]")]
#[case::format("#{pane_id}")]
#[case::command("#(touch /tmp/dbar-pwned)")]
#[case::combined("a#[bold]b#{q:x}c#(id)d")]
fn hostile_values_are_neutralised_in_every_segment(#[case] hostile: &str) {
    let project = ProjectName::new(hostile);
    let status = GitStatus {
        branch: BranchName::new(hostile),
        dirty: false,
        staged: false,
        ahead: AheadCount::new(0),
        behind: BehindCount::new(0),
        is_worktree: false,
    };
    let pr = PrNumber::new(hostile);
    let tmux = TmuxContext {
        session: Some(hostile.to_owned()),
        window: Some(hostile.to_owned()),
        pane: Some(hostile.to_owned()),
        socket: None,
    };
    let context = RenderContext {
        project: &project,
        git_status: Some(&status),
        pr_number: Some(&pr),
        tmux: Some(&tmux),
        clock: Some(hostile),
        client_width: None,
    };
    let line = render_status_line(&context);

    // Scan the way tmux does rather than substring-matching: `##` is a literal
    // `#`, `#[` opens one of the renderer's own style tags, and `#{` or `#(`
    // would be an interpreted format or command — none may survive from
    // attacker-controlled text.
    let mut styles = 0_usize;
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '#' {
            continue;
        }
        match chars.peek() {
            Some('#') => {
                chars.next();
            }
            Some('[') => styles += 1,
            Some(&other) => assert!(
                other != '{' && other != '(',
                "unescaped #{other} introducer survived in: {line}"
            ),
            None => {}
        }
    }
    // The renderer's own style tags are still emitted, unescaped.
    assert!(styles > 0, "renderer style tags were escaped away: {line}");
}
