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
    if context.session.is_some()
        && context.window.is_some()
        && context.pane.is_some()
        && context.socket.is_some()
    {
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
    let mut parts = output.stdout.splitn(4, '|');
    let session = parts.next()?.to_owned();
    let window = parts.next()?.to_owned();
    let pane = parts.next()?.to_owned();
    let socket = parts.next().unwrap_or_default().to_owned();
    Some((session, window, pane, socket))
}
