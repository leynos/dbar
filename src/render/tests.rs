//! Tests for status-line rendering, style tags, and glyph emission.
use super::*;
use crate::tmux::TmuxContext;
use crate::types::{AheadCount, BehindCount, BranchName, PrNumber, ProjectName};
use proptest::prelude::*;
use rstest::{fixture, rstest};
use unicode_width::UnicodeWidthStr;

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
        branch: BranchName::new("main"),
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
struct ConstructScan {
    /// Number of the renderer's own `#[...]` style tags.
    styles: usize,
    /// Introducer of the first surviving `#{` or `#(` construct, if any.
    active: Option<char>,
}

/// Scan a rendered line the way tmux does.
///
/// `##` is a literal `#`, `#[` opens one of the renderer's own style tags, and
/// `#{` or `#(` would be an interpreted format or command.
fn scan_constructs(line: &str) -> ConstructScan {
    let mut styles = 0_usize;
    let mut active = None;
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
            Some(&other) if other == '{' || other == '(' => active = active.or(Some(other)),
            Some(_) | None => {}
        }
    }
    ConstructScan { styles, active }
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
    // The renderer's own style tags are still emitted, unescaped.
    assert!(
        scan.styles > 0,
        "renderer style tags were escaped away: {line}"
    );
}

/// Attacker-controlled values for every dynamic field of the status line.
struct DynamicValues {
    /// Project name for the left-most segment.
    project: String,
    /// Git branch name.
    branch: String,
    /// Pull-request number.
    pr: String,
    /// Clock label.
    clock: String,
    /// tmux session name.
    session: String,
    /// tmux socket path.
    socket: String,
}

impl DynamicValues {
    /// Use one value for every field.
    fn uniform(value: &str) -> Self {
        Self {
            project: value.to_owned(),
            branch: value.to_owned(),
            pr: value.to_owned(),
            clock: value.to_owned(),
            session: value.to_owned(),
            socket: value.to_owned(),
        }
    }
}

/// Render a full status line from attacker-controlled values.
fn render_dynamic(values: &DynamicValues) -> String {
    let project = ProjectName::new(values.project.clone());
    let status = GitStatus {
        branch: BranchName::new(values.branch.clone()),
        dirty: false,
        staged: false,
        ahead: AheadCount::new(0),
        behind: BehindCount::new(0),
        is_worktree: false,
    };
    let pr = PrNumber::new(values.pr.clone());
    let tmux = TmuxContext {
        session: Some(values.session.clone()),
        window: Some("1".into()),
        pane: Some("%0".into()),
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
fn dynamic_value() -> impl Strategy<Value = String> {
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

proptest! {
    // Bounded and deterministic for continuous integration; regression files
    // are disabled because the repository tracks none.
    #![proptest_config(ProptestConfig {
        cases: 256,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    /// No dynamic value may introduce an active `#{` or `#(` construct, while
    /// the renderer's own `#[...]` style tags survive unescaped.
    #[test]
    fn dynamic_values_never_introduce_active_constructs(
        project in dynamic_value(),
        branch in dynamic_value(),
        pr in dynamic_value(),
        clock in dynamic_value(),
        session in dynamic_value(),
        socket in dynamic_value(),
    ) {
        let line = render_dynamic(&DynamicValues { project, branch, pr, clock, session, socket });
        let scan = scan_constructs(&line);
        prop_assert!(scan.active.is_none(), "active construct survived in: {line}");
        prop_assert!(scan.styles > 0, "renderer style tags were escaped away: {line}");
    }

    /// Escaping preserves the value's visible width: `##` renders as one `#`.
    #[test]
    fn escaping_preserves_visible_width(value in dynamic_value()) {
        prop_assert_eq!(
            visible_width(&escape_tmux(&value)),
            UnicodeWidthStr::width(value.as_str())
        );
    }

    /// Width accounting is exact enough to pad a line to the client width.
    #[test]
    fn layout_pads_escaped_values_to_the_client_width(
        left in dynamic_value(),
        right in dynamic_value(),
        slack in 1_usize..40,
    ) {
        let escaped_left = escape_tmux(&left);
        let escaped_right = escape_tmux(&right);
        let width = visible_width(&escaped_left) + visible_width(&escaped_right) + slack + 1;
        let output = layout_with_width(&escaped_left, &escaped_right, width);
        prop_assert_eq!(visible_width(&output), width);
    }
}
