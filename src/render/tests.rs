//! Tests for status-line rendering, style tags, and glyph emission.
use super::*;
use crate::tmux::TmuxContext;
use crate::types::{AheadCount, BehindCount, BranchName, PrNumber, ProjectName};
use proptest::prelude::*;
use rstest::{fixture, rstest};

/// tmux metadata for a pane on the default server.
#[fixture]
fn tmux_context() -> TmuxContext {
    TmuxContext {
        session: Some("session".into()),
        window: Some("1".into()),
        pane: Some("%0".into()),
        socket: None,
    }
}

#[rstest]
fn render_includes_branch_and_pr(tmux_context: TmuxContext) {
    let project = ProjectName::new("demo");
    let status = GitStatus {
        branch: Some(BranchName::new("main")),
        dirty: false,
        staged: false,
        ahead: AheadCount::new(0),
        behind: BehindCount::new(0),
        is_worktree: false,
    };
    let pr = PrNumber::new("17");
    let context = RenderContext {
        project: &project,
        git_status: Some(&status),
        pr_number: Some(&pr),
        tmux: Some(&tmux_context),
        clock: None,
        client_width: None,
    };
    let line = render_status_line(&context);
    assert!(line.contains("main"));
    assert!(line.contains("#17"));
}

#[rstest]
fn render_labels_an_absent_branch_as_detached(tmux_context: TmuxContext) {
    let project = ProjectName::new("demo");
    let status = GitStatus {
        branch: None,
        dirty: false,
        staged: false,
        ahead: AheadCount::new(0),
        behind: BehindCount::new(0),
        is_worktree: false,
    };
    let context = RenderContext {
        project: &project,
        git_status: Some(&status),
        pr_number: None,
        tmux: Some(&tmux_context),
        clock: None,
        client_width: None,
    };
    let line = render_status_line(&context);
    // The rendered contract is unchanged: a detached HEAD still draws the
    // branch glyph followed by the literal label.
    assert!(line.contains(GLYPH_BRANCH));
    assert!(line.contains("detached"));
}

#[test]
fn layout_right_justifies_with_width() {
    let output = layout_with_width("left", "right", 12);
    assert_eq!(output, "left   right");
}

#[rstest]
fn render_places_clock_after_tmux_on_right(tmux_context: TmuxContext) {
    let project = ProjectName::new("demo");
    let context = RenderContext {
        project: &project,
        git_status: None,
        pr_number: None,
        tmux: Some(&tmux_context),
        clock: Some("09:41"),
        client_width: None,
    };
    let line = render_status_line(&context);
    assert!(line.contains("session:1.%0"));
    assert!(line.ends_with(" 09:41#[default]"));
}

/// The socket names the server, and only when it is not tmux's default one.
#[rstest]
#[case::absent(None, "session:1.%0")]
#[case::named(Some("/tmp/tmux-1000/build"), "session:1.%0@build")]
#[case::default_server(Some("/tmp/tmux-1000/default"), "session:1.%0")]
#[case::bare_name(Some("build"), "session:1.%0@build")]
#[case::hostile(Some("/tmp/#{pane_id}"), "session:1.%0@##{pane_id}")]
fn render_tmux_segment_labels_the_socket(
    mut tmux_context: TmuxContext,
    #[case] socket: Option<&str>,
    #[case] expected: &str,
) {
    tmux_context.socket = socket.map(std::borrow::ToOwned::to_owned);
    let segment = render_tmux_segment(&tmux_context);
    assert!(
        segment
            .as_deref()
            .is_some_and(|text| text.contains(expected)),
        "expected {expected:?} in {segment:?}"
    );
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

/// What a tmux-style scan of a rendered line found.
pub(super) struct ConstructScan {
    /// Number of the renderer's own `#[...]` style tags.
    pub(super) styles: usize,
    /// Introducer of the first surviving `#{` or `#(` construct, if any.
    pub(super) active: Option<char>,
    /// Body of the first `#[...]` tag outside the renderer's vocabulary.
    pub(super) foreign: Option<String>,
}

/// Whether one `fg=`/`bg=` clause is one the renderer itself emits.
///
/// `style` emits `fg=colourN` and `bg=colourN`, and `reset_bg_with_fg` emits
/// `bg=default`. Nothing else — notably no named colour such as `red` — is
/// renderer-owned, so anything else must have come from a dynamic value.
fn is_renderer_clause(clause: &str) -> bool {
    fn is_indexed_colour(value: &str) -> bool {
        value
            .strip_prefix("colour")
            .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
    }

    match clause.split_once('=') {
        Some(("fg", value)) => is_indexed_colour(value),
        Some(("bg", value)) => value == "default" || is_indexed_colour(value),
        Some(_) | None => false,
    }
}

/// Whether a `#[...]` body is one the renderer itself emits.
///
/// The full vocabulary is `default` (from `reset` and `style(None, None)`) and
/// comma-separated `fg`/`bg` clauses (from `style` and `reset_bg_with_fg`).
fn is_renderer_style(body: &str) -> bool {
    body == "default" || body.split(',').all(is_renderer_clause)
}

/// Scan a rendered line the way tmux does.
///
/// `##` is a literal `#`, `#[` opens a style tag, and `#{` or `#(` would be an
/// interpreted format or command. A style tag is only counted as the
/// renderer's own when its body matches the renderer's vocabulary; any other
/// body must have arrived through a dynamic value that escaping should have
/// neutralised, and is reported through `foreign` instead. Without that check
/// an injected `#[fg=red]` would satisfy the "style tags survive" assertion and
/// mask a missing `escape_tmux` call.
pub(super) fn scan_constructs(line: &str) -> ConstructScan {
    let mut styles = 0_usize;
    let mut active = None;
    let mut foreign: Option<String> = None;
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '#' {
            continue;
        }
        match chars.peek() {
            Some('#') => {
                chars.next();
            }
            Some('[') => {
                chars.next();
                let body: String = chars.by_ref().take_while(|next| *next != ']').collect();
                if is_renderer_style(&body) {
                    styles += 1;
                } else {
                    foreign = foreign.or(Some(body));
                }
            }
            Some(&other) if other == '{' || other == '(' => active = active.or(Some(other)),
            Some(_) | None => {}
        }
    }
    ConstructScan {
        styles,
        active,
        foreign,
    }
}

#[test]
pub(super) fn scan_constructs_rejects_value_derived_style_tags() {
    let renderer_only = scan_constructs("#[fg=colour117,bg=colour24]x#[fg=colour24,bg=default]y");
    assert_eq!(renderer_only.styles, 2);
    assert_eq!(renderer_only.foreign, None);

    let injected = scan_constructs("#[default]a#[fg=red]b");
    assert_eq!(injected.styles, 1);
    assert_eq!(injected.foreign.as_deref(), Some("fg=red"));

    // An escaped tag is a literal `#` followed by plain text, not a tag.
    let escaped = scan_constructs("##[fg=red]");
    assert_eq!(escaped.styles, 0);
    assert_eq!(escaped.foreign, None);
}

/// Hostile values must not introduce unescaped tmux markup.
#[rstest]
#[case::style("#[fg=red]")]
#[case::format("#{pane_id}")]
#[case::command("#(touch /tmp/dbar-pwned)")]
#[case::combined("a#[bold]b#{q:x}c#(id)d")]
fn hostile_values_are_neutralised_in_every_segment(#[case] hostile: &str) {
    let line = render_dynamic(&DynamicValues::uniform(hostile));
    let scan = scan_constructs(&line);

    assert!(
        scan.active.is_none(),
        "unescaped introducer survived in: {line}"
    );
    assert_eq!(
        scan.foreign, None,
        "a value-derived style tag survived in: {line}"
    );
    // The renderer's own style tags are still emitted, unescaped.
    assert!(
        scan.styles > 0,
        "renderer style tags were escaped away: {line}"
    );
}

/// Attacker-controlled values for every dynamic field of the status line.
pub(super) struct DynamicValues {
    /// Project name for the left-most segment.
    pub(super) project: String,
    /// Git branch name.
    pub(super) branch: String,
    /// Pull-request number.
    pub(super) pr: String,
    /// Clock label.
    pub(super) clock: String,
    /// tmux session name.
    pub(super) session: String,
    /// tmux window index.
    ///
    /// tmux supplies this, but dbar receives it as a command-line argument and
    /// interpolates it into the same location label as the session, so it is
    /// no less attacker-influenced than the rest and is generated, not pinned.
    pub(super) window: String,
    /// tmux pane identifier, carried for the same reason as `window`.
    pub(super) pane: String,
    /// tmux socket path.
    pub(super) socket: String,
}

impl DynamicValues {
    /// Use one value for every field.
    pub(super) fn uniform(value: &str) -> Self {
        Self {
            project: value.to_owned(),
            branch: value.to_owned(),
            pr: value.to_owned(),
            clock: value.to_owned(),
            session: value.to_owned(),
            window: value.to_owned(),
            pane: value.to_owned(),
            socket: value.to_owned(),
        }
    }
}

/// Render a full status line from attacker-controlled values.
pub(super) fn render_dynamic(values: &DynamicValues) -> String {
    let project = ProjectName::new(values.project.clone());
    let status = GitStatus {
        branch: Some(BranchName::new(values.branch.clone())),
        dirty: false,
        staged: false,
        ahead: AheadCount::new(0),
        behind: BehindCount::new(0),
        is_worktree: false,
    };
    let pr = PrNumber::new(values.pr.clone());
    let tmux = TmuxContext {
        session: Some(values.session.clone()),
        window: Some(values.window.clone()),
        pane: Some(values.pane.clone()),
        socket: Some(values.socket.clone()),
    };
    let context = RenderContext {
        project: &project,
        git_status: Some(&status),
        pr_number: Some(&pr),
        tmux: Some(&tmux),
        clock: Some(&values.clock),
        client_width: None,
    };
    render_status_line(&context)
}

/// tmux and shell metacharacters that could open a construct.
const META_CHARS: &[char] = &[
    '#', '{', '}', '[', ']', '(', ')', '$', '`', '\'', '"', ';', '|', '&', '\\', '%', ':', '@',
];
/// Characters occupying two columns.
const WIDE_CHARS: &[char] = &['漢', '字', '한', '　', 'ｗ', 'あ'];
/// Combining marks and zero-width characters, all occupying no columns.
const MARK_CHARS: &[char] = &['\u{300}', '\u{301}', '\u{35b}', '\u{200b}', '\u{200c}'];

/// A bounded generator of hostile, Unicode-mixed renderer input.
pub(super) fn dynamic_value() -> impl Strategy<Value = String> {
    let character = prop_oneof![
        4 => proptest::char::range('!', '~'),
        4 => proptest::sample::select(META_CHARS),
        2 => proptest::sample::select(WIDE_CHARS),
        2 => proptest::sample::select(MARK_CHARS),
        1 => proptest::char::range('\u{a1}', '\u{17f}'),
    ];
    proptest::collection::vec(character, 0..12)
        .prop_map(|characters| characters.into_iter().collect())
}
