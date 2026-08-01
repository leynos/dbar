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

fn query_tmux(runner: &dyn CommandRunner) -> Option<(String, String, String, String)> {
    let spec = CommandSpec::new("tmux").args([
        "display-message",
        "-p",
        "#{session_name}|#{window_index}|#{pane_id}|#{socket_path}",
    ]);
    let output = runner.run(&spec).ok()?;
    let mut parts = output.stdout.trim().splitn(4, '|');
    let session = parts.next()?.to_owned();
    let window = parts.next()?.to_owned();
    let pane = parts.next()?.to_owned();
    let socket = parts.next().unwrap_or_default().to_owned();
    Some((session, window, pane, socket))
}

#[cfg(test)]
mod tests {
    //! Tests for tmux context resolution and `display-message` parsing.
    use super::*;
    use crate::command::{CommandError, CommandOutput};
    use rstest::rstest;

    /// A runner that returns one canned stdout, or fails when none is set, and
    /// counts how many times it was consulted.
    #[derive(Default)]
    struct StubRunner {
        stdout: Option<String>,
        calls: std::cell::Cell<usize>,
    }

    impl StubRunner {
        fn with_stdout(stdout: &str) -> Self {
            Self {
                stdout: Some(stdout.to_owned()),
                ..Self::default()
            }
        }
    }

    impl CommandRunner for StubRunner {
        fn run(&self, _spec: &CommandSpec) -> Result<CommandOutput, CommandError> {
            self.calls.set(self.calls.get() + 1);
            self.stdout.as_ref().map_or(
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
        let runner = StubRunner::with_stdout("sess|1|%0|/tmp/sock\n");
        let resolved = resolve_context(&runner, TmuxContext::default());
        assert_eq!(resolved.session.as_deref(), Some("sess"));
        assert_eq!(resolved.window.as_deref(), Some("1"));
        assert_eq!(resolved.pane.as_deref(), Some("%0"));
        // The trailing newline must not survive into the socket field.
        assert_eq!(resolved.socket.as_deref(), Some("/tmp/sock"));
    }

    #[rstest]
    fn resolve_context_preserves_prepopulated_fields() {
        let runner = StubRunner::with_stdout("other|9|%9|/tmp/other");
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
        let runner = StubRunner::with_stdout("sess||%0|");
        let resolved = resolve_context(&runner, TmuxContext::default());
        assert_eq!(resolved.session.as_deref(), Some("sess"));
        assert_eq!(resolved.window, None);
        assert_eq!(resolved.pane.as_deref(), Some("%0"));
        assert_eq!(resolved.socket, None);
    }

    #[rstest]
    #[case::short_output("sess|1")]
    #[case::empty_output("")]
    fn resolve_context_handles_malformed_output(#[case] stdout: &str) {
        let runner = StubRunner::with_stdout(stdout);
        let resolved = resolve_context(&runner, TmuxContext::default());
        // Malformed output must leave the context untouched, not panic.
        assert_eq!(resolved.session, None);
        assert_eq!(resolved.window, None);
    }

    #[rstest]
    fn resolve_context_tolerates_command_failure() {
        let runner = StubRunner::default();
        let resolved = resolve_context(&runner, TmuxContext::default());
        assert_eq!(resolved.session, None);
    }
}
