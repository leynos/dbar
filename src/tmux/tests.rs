//! Tests for tmux context resolution, `display-message` parsing, and the
//! typed outcomes for pre-resolved contexts, empty answers, and failures.

use super::*;
use crate::command::{CommandError, CommandOutput};
use rstest::rstest;
use std::cell::Cell;
use std::collections::HashMap;

/// A runner that maps each requested `display-message` specification to a
/// canned stdout, failing for unknown specifications, and counting how many
/// times it was consulted.
#[derive(Default)]
struct StubRunner {
    outputs: HashMap<CommandSpec, String>,
    calls: Cell<usize>,
}

impl StubRunner {
    /// Answer all four tmux fields with the given raw stdout values.
    fn with_fields(session: &str, window: &str, pane: &str, socket: &str) -> Self {
        let mut outputs = HashMap::new();
        outputs.insert(field_spec(TmuxField::Session.format()), session.to_owned());
        outputs.insert(field_spec(TmuxField::Window.format()), window.to_owned());
        outputs.insert(field_spec(TmuxField::Pane.format()), pane.to_owned());
        outputs.insert(field_spec(TmuxField::Socket.format()), socket.to_owned());
        Self {
            outputs,
            ..Self::default()
        }
    }

    /// Drop one field so its query fails, simulating a partial answer.
    fn without_field(mut self, field: TmuxField) -> Self {
        self.outputs.remove(&field_spec(field.format()));
        self
    }
}

impl CommandRunner for StubRunner {
    fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, CommandError> {
        self.calls.set(self.calls.get() + 1);
        self.outputs.get(spec).map_or(
            Err(CommandError::NonZero {
                status: Some(1),
                stderr: String::new(),
            }),
            |stdout| {
                Ok(CommandOutput {
                    stdout: stdout.clone(),
                })
            },
        )
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
    // the runner were called and simply errored.
    let runner = StubRunner::default();
    let context = context_of("sess", "1", "%0", "/tmp/sock");
    let resolution = resolve_context(&runner, context);
    assert!(matches!(resolution.outcome, TmuxOutcome::PreResolved));
    assert_eq!(resolution.context.session.as_deref(), Some("sess"));
    assert_eq!(resolution.context.socket.as_deref(), Some("/tmp/sock"));
    assert_eq!(runner.calls.get(), 0);
}

#[rstest]
fn resolve_context_fills_missing_fields() {
    let runner = StubRunner::with_fields("sess", "1", "%0", "/tmp/sock\n");
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
    let runner = StubRunner::with_fields("other", "9", "%9", "/tmp/other");
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
    let runner = StubRunner::with_fields("sess", "", "%0", "");
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
    let runner = StubRunner::with_fields("sess", "1", "%0", "/tmp/sock").without_field(missing);
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
    let runner = StubRunner::with_fields("a|b", "3", "%2", "/tmp/sock");
    let resolution = resolve_context(&runner, TmuxContext::default());
    assert_eq!(resolution.context.session.as_deref(), Some("a|b"));
    assert_eq!(resolution.context.window.as_deref(), Some("3"));
    assert_eq!(resolution.context.pane.as_deref(), Some("%2"));
    assert_eq!(resolution.context.socket.as_deref(), Some("/tmp/sock"));
}

#[rstest]
fn resolve_context_handles_socket_path_containing_a_pipe() {
    let runner = StubRunner::with_fields("sess", "3", "%2", "/tmp/weird|socket");
    let resolution = resolve_context(&runner, TmuxContext::default());
    assert_eq!(resolution.context.session.as_deref(), Some("sess"));
    assert_eq!(
        resolution.context.socket.as_deref(),
        Some("/tmp/weird|socket")
    );
}

#[rstest]
fn resolve_context_reports_an_unavailable_server() {
    let runner = StubRunner::default();
    let resolution = resolve_context(&runner, TmuxContext::default());
    assert_eq!(resolution.context.session, None);
    // The runner must have been consulted, otherwise this test would pass
    // even if `resolve_context` never queried tmux at all.
    assert!(runner.calls.get() > 0);
    assert!(matches!(
        resolution.outcome,
        TmuxOutcome::Unavailable(TmuxProbeFailure::CommandFailed {
            field: TmuxField::Session,
            ..
        })
    ));
}
