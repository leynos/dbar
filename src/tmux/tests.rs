//! Tests for tmux context resolution, `display-message` parsing, and the
//! typed outcomes for pre-resolved contexts, empty answers, and failures.

use super::*;
use crate::command::{CommandError, CommandOutput, MockCommandRunner};
use mockall::predicate::eq;
use rstest::rstest;

/// The `display-message` format for one field, written out by hand.
///
/// These literals are deliberately *not* derived from [`TmuxField::format`];
/// they are the format strings tmux itself documents. Every mock expectation
/// below is keyed on a specification built from this table, so a typo
/// introduced into the production formats no longer moves both sides of the
/// comparison at once: it stops matching the expectation and so fails the
/// resolution tests too, not merely the literal check in
/// `field_formats_match_their_hand_written_literals`.
const fn expected_format(field: TmuxField) -> &'static str {
    match field {
        TmuxField::Session => "#{session_name}",
        TmuxField::Window => "#{window_index}",
        TmuxField::Pane => "#{pane_id}",
        TmuxField::Socket => "#{socket_path}",
    }
}

/// The `display-message` invocation a field must produce, built from the
/// hand-written literal rather than from `field_spec`.
fn expected_spec(field: TmuxField) -> CommandSpec {
    CommandSpec::new("tmux").args(["display-message", "-p", expected_format(field)])
}

#[rstest]
#[case::session(TmuxField::Session)]
#[case::window(TmuxField::Window)]
#[case::pane(TmuxField::Pane)]
#[case::socket(TmuxField::Socket)]
fn field_formats_match_their_hand_written_literals(#[case] field: TmuxField) {
    // Pinned independently of the production table, so drifting from tmux's
    // documented formats is a bug even when the rest of the crate agrees
    // with the drift.
    let format = expected_format(field);
    assert_eq!(field.format(), format);
    assert_eq!(field.to_string(), format);
    assert_eq!(
        field_spec(field.format()),
        CommandSpec::new("tmux").args(["display-message", "-p", format])
    );
}

/// The failure a query gets when tmux cannot answer it.
fn query_failure() -> CommandError {
    CommandError::NonZero { status: Some(1) }
}

/// The canned `display-message` answers for the four tmux fields; a `None`
/// answer makes that field's query fail, as a dead server would.
///
/// Each field gets its own `expect_run` keyed by an `eq` matcher on the exact
/// specification, so a query for the wrong format string matches nothing and
/// fails the test rather than being silently answered. The specification comes
/// from [`expected_spec`], which is built from hand-written literals, so these
/// resolution tests fail on a production format typo instead of following it.
struct Answers(Vec<(TmuxField, Option<String>)>);

impl Answers {
    /// Answer all four tmux fields with the given raw stdout values.
    fn with_fields(session: &str, window: &str, pane: &str, socket: &str) -> Self {
        Self(vec![
            (TmuxField::Session, Some(session.to_owned())),
            (TmuxField::Window, Some(window.to_owned())),
            (TmuxField::Pane, Some(pane.to_owned())),
            (TmuxField::Socket, Some(socket.to_owned())),
        ])
    }

    /// Drop one field so its query fails, simulating a partial answer.
    fn without_field(mut self, field: TmuxField) -> Self {
        for (name, answer) in &mut self.0 {
            if *name == field {
                *answer = None;
            }
        }
        self
    }

    /// Build a mock runner carrying one expectation per field.
    fn build(self) -> MockCommandRunner {
        let mut runner = MockCommandRunner::new();
        for (field, answer) in self.0 {
            runner
                .expect_run()
                .with(eq(expected_spec(field)))
                .returning(move |_| {
                    answer.clone().map_or_else(
                        || Err(query_failure()),
                        |stdout| Ok(CommandOutput { stdout }),
                    )
                });
        }
        runner
    }
}

fn context_of(session: &str, window: &str, pane: &str, socket: &str) -> TmuxContext {
    TmuxContext {
        session: Some(session.to_owned()),
        window: Some(window.to_owned()),
        pane: Some(pane.to_owned()),
        socket: Some(socket.to_owned()),
    }
}

#[rstest]
fn resolve_context_short_circuits_when_complete() {
    // Asserting the runner was never consulted is what proves the
    // short-circuit: returning the context unchanged would also happen if
    // the runner were called and simply errored. `never()` fails the test on
    // the first query rather than after the fact.
    let mut runner = MockCommandRunner::new();
    runner.expect_run().never();
    let context = context_of("sess", "1", "%0", "/tmp/sock");
    let resolution = resolve_context(&runner, context);
    assert!(matches!(resolution.outcome, TmuxOutcome::PreResolved));
    assert_eq!(resolution.context.session.as_deref(), Some("sess"));
    assert_eq!(resolution.context.socket.as_deref(), Some("/tmp/sock"));
}

#[rstest]
fn resolve_context_fills_missing_fields() {
    let runner = Answers::with_fields("sess", "1", "%0", "/tmp/sock\n").build();
    let resolution = resolve_context(&runner, TmuxContext::default());
    assert_eq!(resolution.context.session.as_deref(), Some("sess"));
    assert_eq!(resolution.context.window.as_deref(), Some("1"));
    assert_eq!(resolution.context.pane.as_deref(), Some("%0"));
    // The trailing newline must not survive into the socket field.
    assert_eq!(resolution.context.socket.as_deref(), Some("/tmp/sock"));
    assert!(resolution.into_failures().is_empty());
}

#[rstest]
fn resolve_context_preserves_prepopulated_fields() {
    let mut runner = MockCommandRunner::new();
    for (field, stdout) in [
        (TmuxField::Window, "9"),
        (TmuxField::Pane, "%9"),
        (TmuxField::Socket, "/tmp/other"),
    ] {
        runner
            .expect_run()
            .with(eq(expected_spec(field)))
            .times(1)
            .returning(move |_| {
                Ok(CommandOutput {
                    stdout: stdout.to_owned(),
                })
            });
    }
    let context = TmuxContext {
        session: Some("mine".to_owned()),
        ..TmuxContext::default()
    };
    let resolution = resolve_context(&runner, context);
    assert_eq!(resolution.context.session.as_deref(), Some("mine"));
    assert_eq!(resolution.context.window.as_deref(), Some("9"));
}

#[rstest]
fn resolve_context_reports_empty_response_fields() {
    let runner = Answers::with_fields("sess", "", "%0", "").build();
    let resolution = resolve_context(&runner, TmuxContext::default());
    assert_eq!(resolution.context.session.as_deref(), Some("sess"));
    // The rendered contract is unchanged: an empty answer leaves the field
    // unset rather than rendering an empty value ...
    assert_eq!(resolution.context.window, None);
    assert_eq!(resolution.context.pane.as_deref(), Some("%0"));
    assert_eq!(resolution.context.socket, None);

    // ... and both empty answers are now reported.
    let failures = resolution.into_failures();
    assert_eq!(failures.len(), 2);
    assert!(failures.iter().any(|failure| matches!(
        failure,
        TmuxProbeFailure::EmptyValue {
            field: TmuxField::Window
        }
    )));
    assert!(failures.iter().any(|failure| matches!(
        failure,
        TmuxProbeFailure::EmptyValue {
            field: TmuxField::Socket
        }
    )));
}

#[rstest]
#[case::session(TmuxField::Session)]
#[case::pane(TmuxField::Pane)]
fn resolve_context_reports_a_failed_query(#[case] missing: TmuxField) {
    let runner = Answers::with_fields("sess", "1", "%0", "/tmp/sock")
        .without_field(missing)
        .build();
    let resolution = resolve_context(&runner, TmuxContext::default());
    // A partial answer must leave the context untouched, as before ...
    assert_eq!(resolution.context.session, None);
    assert_eq!(resolution.context.window, None);

    // ... and name the field whose query failed.
    let failures = resolution.into_failures();
    match <[TmuxProbeFailure; 1]>::try_from(failures) {
        Ok([failure]) => assert!(matches!(
            failure,
            TmuxProbeFailure::CommandFailed { field, .. } if field == missing
        )),
        Err(other) => panic!("expected exactly one failure, got {other:?}"),
    }
}

#[rstest]
fn resolve_context_handles_session_name_containing_a_pipe() {
    // tmux forbids only `:` and `.` in session names, so `|` is legal and
    // must not be mistaken for a field separator.
    let runner = Answers::with_fields("a|b", "3", "%2", "/tmp/sock").build();
    let resolution = resolve_context(&runner, TmuxContext::default());
    assert_eq!(resolution.context.session.as_deref(), Some("a|b"));
    assert_eq!(resolution.context.window.as_deref(), Some("3"));
    assert_eq!(resolution.context.pane.as_deref(), Some("%2"));
    assert_eq!(resolution.context.socket.as_deref(), Some("/tmp/sock"));
}

#[rstest]
fn resolve_context_handles_socket_path_containing_a_pipe() {
    let runner = Answers::with_fields("sess", "3", "%2", "/tmp/weird|socket").build();
    let resolution = resolve_context(&runner, TmuxContext::default());
    assert_eq!(resolution.context.session.as_deref(), Some("sess"));
    assert_eq!(
        resolution.context.socket.as_deref(),
        Some("/tmp/weird|socket")
    );
}

#[rstest]
fn resolve_context_reports_an_unavailable_server() {
    // The runner must have been consulted, otherwise this test would pass
    // even if `resolve_context` never queried tmux at all; `times(1..)` is
    // that assertion, checked when the mock is dropped.
    let mut runner = MockCommandRunner::new();
    runner
        .expect_run()
        .times(1..)
        .returning(|_| Err(query_failure()));
    let resolution = resolve_context(&runner, TmuxContext::default());
    assert_eq!(resolution.context.session, None);
    assert!(matches!(
        resolution.outcome,
        TmuxOutcome::Unavailable(TmuxProbeFailure::CommandFailed {
            field: TmuxField::Session,
            ..
        })
    ));
}
