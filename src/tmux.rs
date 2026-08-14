//! tmux context extraction helpers.

use crate::command::{CommandRunner, CommandSpec};

#[derive(Debug, Clone, Default)]
/// tmux metadata passed into the status renderer.
pub struct TmuxContext {
    /// tmux session name.
    pub session: Option<String>,
    /// tmux window index.
    pub window: Option<String>,
    /// tmux pane id.
    pub pane: Option<String>,
    /// tmux socket path.
    pub socket: Option<String>,
}

impl TmuxContext {
    /// Report whether every tmux field has already been resolved.
    const fn is_complete(&self) -> bool {
        self.session.is_some()
            && self.window.is_some()
            && self.pane.is_some()
            && self.socket.is_some()
    }
}

/// Fill missing tmux fields by querying the tmux server.
///
/// # Examples
///
/// ```rust,ignore
/// use dbar::command::RealCommandRunner;
/// use dbar::tmux::{resolve_context, TmuxContext};
///
/// let runner = RealCommandRunner::default();
/// let context = resolve_context(&runner, TmuxContext::default());
/// let _ = context.session;
/// ```
pub fn resolve_context(runner: &dyn CommandRunner, mut context: TmuxContext) -> TmuxContext {
    if context.is_complete() {
        return context;
    }

    let Some((session, window, pane, socket)) = query_tmux(runner) else {
        return context;
    };

    if context.session.is_none() && !session.is_empty() {
        context.session = Some(session);
    }
    if context.window.is_none() && !window.is_empty() {
        context.window = Some(window);
    }
    if context.pane.is_none() && !pane.is_empty() {
        context.pane = Some(pane);
    }
    if context.socket.is_none() && !socket.is_empty() {
        context.socket = Some(socket);
    }

    context
}

/// Build the `display-message` specification for a single tmux format.
fn field_spec(format: &str) -> CommandSpec {
    CommandSpec::new("tmux").args(["display-message", "-p", format])
}

/// Query one tmux format string, returning its trimmed value.
///
/// Each call's entire trimmed stdout *is* the field value, so no separator
/// character has to be reserved and no byte content can misalign the result.
fn query_field(runner: &dyn CommandRunner, format: &str) -> Option<String> {
    let output = runner.run(&field_spec(format)).ok()?;
    Some(output.stdout.trim().to_owned())
}

/// Query the four tmux fields, one `display-message` invocation apiece.
///
/// Packing the fields into a single delimited format string would be cheaper,
/// but no delimiter is safe: tmux only forbids `:` and `.` in session names, so
/// a session called `a|b` (or one containing any other candidate separator)
/// would shift every subsequent field. The extra processes are affordable
/// because this is a fallback path, not the per-refresh hot path:
/// `resolve_context` short-circuits via `is_complete`, and the installed tmux
/// snippet normally passes `--session/--window/--pane/--socket` explicitly.
fn query_tmux(runner: &dyn CommandRunner) -> Option<(String, String, String, String)> {
    let session = query_field(runner, "#{session_name}")?;
    let window = query_field(runner, "#{window_index}")?;
    let pane = query_field(runner, "#{pane_id}")?;
    let socket = query_field(runner, "#{socket_path}")?;
    Some((session, window, pane, socket))
}

#[cfg(test)]
mod tests {
    //! Tests for tmux context resolution and `display-message` parsing.
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
            outputs.insert(field_spec("#{session_name}"), session.to_owned());
            outputs.insert(field_spec("#{window_index}"), window.to_owned());
            outputs.insert(field_spec("#{pane_id}"), pane.to_owned());
            outputs.insert(field_spec("#{socket_path}"), socket.to_owned());
            Self {
                outputs,
                ..Self::default()
            }
        }

        /// Drop one field so its query fails, simulating a partial answer.
        fn without_field(mut self, format: &str) -> Self {
            self.outputs.remove(&field_spec(format));
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
        let resolved = resolve_context(&runner, context);
        assert_eq!(resolved.session.as_deref(), Some("sess"));
        assert_eq!(resolved.socket.as_deref(), Some("/tmp/sock"));
        assert_eq!(runner.calls.get(), 0);
    }

    #[rstest]
    fn resolve_context_fills_missing_fields() {
        let runner = StubRunner::with_fields("sess", "1", "%0", "/tmp/sock\n");
        let resolved = resolve_context(&runner, TmuxContext::default());
        assert_eq!(resolved.session.as_deref(), Some("sess"));
        assert_eq!(resolved.window.as_deref(), Some("1"));
        assert_eq!(resolved.pane.as_deref(), Some("%0"));
        // The trailing newline must not survive into the socket field.
        assert_eq!(resolved.socket.as_deref(), Some("/tmp/sock"));
    }

    #[rstest]
    fn resolve_context_preserves_prepopulated_fields() {
        let runner = StubRunner::with_fields("other", "9", "%9", "/tmp/other");
        let context = TmuxContext {
            session: Some("mine".to_owned()),
            ..TmuxContext::default()
        };
        let resolved = resolve_context(&runner, context);
        assert_eq!(resolved.session.as_deref(), Some("mine"));
        assert_eq!(resolved.window.as_deref(), Some("9"));
    }

    #[rstest]
    fn resolve_context_ignores_empty_response_fields() {
        let runner = StubRunner::with_fields("sess", "", "%0", "");
        let resolved = resolve_context(&runner, TmuxContext::default());
        assert_eq!(resolved.session.as_deref(), Some("sess"));
        assert_eq!(resolved.window, None);
        assert_eq!(resolved.pane.as_deref(), Some("%0"));
        assert_eq!(resolved.socket, None);
    }

    #[rstest]
    #[case::partial_answer(StubRunner::with_fields("sess", "1", "%0", "/tmp/sock")
        .without_field("#{pane_id}"))]
    #[case::empty_answers(StubRunner::with_fields("", "", "", ""))]
    fn resolve_context_handles_unusable_output(#[case] runner: StubRunner) {
        let resolved = resolve_context(&runner, TmuxContext::default());
        // Unusable output must leave the context untouched, not panic.
        assert_eq!(resolved.session, None);
        assert_eq!(resolved.window, None);
    }

    #[rstest]
    fn resolve_context_handles_session_name_containing_a_pipe() {
        // tmux forbids only `:` and `.` in session names, so `|` is legal and
        // must not be mistaken for a field separator.
        let runner = StubRunner::with_fields("a|b", "3", "%2", "/tmp/sock");
        let resolved = resolve_context(&runner, TmuxContext::default());
        assert_eq!(resolved.session.as_deref(), Some("a|b"));
        assert_eq!(resolved.window.as_deref(), Some("3"));
        assert_eq!(resolved.pane.as_deref(), Some("%2"));
        assert_eq!(resolved.socket.as_deref(), Some("/tmp/sock"));
    }

    #[rstest]
    fn resolve_context_handles_socket_path_containing_a_pipe() {
        let runner = StubRunner::with_fields("sess", "3", "%2", "/tmp/weird|socket");
        let resolved = resolve_context(&runner, TmuxContext::default());
        assert_eq!(resolved.session.as_deref(), Some("sess"));
        assert_eq!(resolved.window.as_deref(), Some("3"));
        assert_eq!(resolved.pane.as_deref(), Some("%2"));
        assert_eq!(resolved.socket.as_deref(), Some("/tmp/weird|socket"));
    }

    #[rstest]
    fn resolve_context_tolerates_command_failure() {
        let runner = StubRunner::default();
        let resolved = resolve_context(&runner, TmuxContext::default());
        assert_eq!(resolved.session, None);
        // The runner must have been consulted, otherwise this test would pass
        // even if `resolve_context` never queried tmux at all.
        assert!(runner.calls.get() > 0);
    }
}
