//! tmux context extraction helpers.
//!
//! `resolve_context` fills in whatever the caller did not supply by asking the
//! tmux server, and reports what happened as a typed outcome rather than
//! quietly returning a half-populated context.
//!
//! # Fallback policy
//!
//! The rendered status line is unchanged by any of these failures; only the
//! diagnosis differs.
//!
//! | Outcome | Rendered result |
//! | --- | --- |
//! | [`TmuxOutcome::PreResolved`] | the caller's values, tmux is never run |
//! | [`TmuxOutcome::Queried`] with no failures | the caller's values plus every queried value |
//! | [`TmuxOutcome::Queried`] listing empty fields | those fields stay unset, so the renderer omits the tmux segment when the session is one of them |
//! | [`TmuxOutcome::Unavailable`] | no field is filled in, exactly as if tmux had answered nothing |
//!
//! A single failed query aborts the whole probe: tmux answers all four fields
//! from the same client, so a failure part-way through means the answers
//! already collected describe a client that has since gone away.

use std::fmt;

use thiserror::Error;

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

    /// Borrow the slot a queried field is stored in.
    const fn slot_mut(&mut self, field: TmuxField) -> &mut Option<String> {
        match field {
            TmuxField::Session => &mut self.session,
            TmuxField::Window => &mut self.window,
            TmuxField::Pane => &mut self.pane,
            TmuxField::Socket => &mut self.socket,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// One tmux field queried through `display-message`.
pub enum TmuxField {
    /// The session name.
    Session,
    /// The window index.
    Window,
    /// The pane id.
    Pane,
    /// The server socket path.
    Socket,
}

impl TmuxField {
    /// The tmux format string that yields this field.
    const fn format(self) -> &'static str {
        match self {
            Self::Session => "#{session_name}",
            Self::Window => "#{window_index}",
            Self::Pane => "#{pane_id}",
            Self::Socket => "#{socket_path}",
        }
    }
}

impl fmt::Display for TmuxField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.format())
    }
}

/// The fields queried, in the order they are requested.
const TMUX_FIELDS: [TmuxField; 4] = [
    TmuxField::Session,
    TmuxField::Window,
    TmuxField::Pane,
    TmuxField::Socket,
];

#[derive(Debug, Error)]
/// Why a tmux query did not yield a usable value.
pub enum TmuxProbeFailure {
    /// The `display-message` invocation could not be run or exited non-zero,
    /// which is what a missing tmux or a dead server looks like.
    #[error("`tmux display-message -p {field}` failed: {source}")]
    CommandFailed {
        /// The field being queried when the command failed.
        field: TmuxField,
        /// The underlying command failure.
        source: crate::command::CommandError,
    },
    /// tmux answered, but with an empty value, so the field stays unset.
    #[error("`tmux display-message -p {field}` returned an empty value")]
    EmptyValue {
        /// The field whose value was empty.
        field: TmuxField,
    },
}

#[derive(Debug)]
/// What [`resolve_context`] did to fill the missing fields.
pub enum TmuxOutcome {
    /// Every field was already supplied, so tmux was never queried.
    PreResolved,
    /// tmux was queried; `malformed` lists the fields it left unusable.
    Queried {
        /// Fields that were needed but came back empty.
        malformed: Vec<TmuxProbeFailure>,
    },
    /// A query failed, so no field was filled in.
    Unavailable(TmuxProbeFailure),
}

#[derive(Debug)]
/// The resolved tmux context and the typed record of how it was obtained.
pub struct TmuxResolution {
    /// The context to render.
    pub context: TmuxContext,
    /// What happened while filling the missing fields.
    pub outcome: TmuxOutcome,
}

impl TmuxResolution {
    /// Consume the resolution, returning every failure it recorded.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use dbar::command::RealCommandRunner;
    /// use dbar::tmux::{resolve_context, TmuxContext};
    ///
    /// let runner = RealCommandRunner::default();
    /// let resolution = resolve_context(&runner, TmuxContext::default());
    /// let _ = resolution.into_failures();
    /// ```
    pub fn into_failures(self) -> Vec<TmuxProbeFailure> {
        match self.outcome {
            TmuxOutcome::PreResolved => Vec::new(),
            TmuxOutcome::Queried { malformed } => malformed,
            TmuxOutcome::Unavailable(failure) => vec![failure],
        }
    }
}

/// Fill missing tmux fields by querying the tmux server.
///
/// The module-level fallback policy describes what each outcome renders.
///
/// # Examples
///
/// ```rust,ignore
/// use dbar::command::RealCommandRunner;
/// use dbar::tmux::{resolve_context, TmuxContext};
///
/// let runner = RealCommandRunner::default();
/// let resolution = resolve_context(&runner, TmuxContext::default());
/// let _ = resolution.context.session;
/// ```
pub fn resolve_context(runner: &dyn CommandRunner, context: TmuxContext) -> TmuxResolution {
    if context.is_complete() {
        return TmuxResolution {
            context,
            outcome: TmuxOutcome::PreResolved,
        };
    }

    match query_fields(runner) {
        Ok(values) => merge_fields(context, values),
        Err(failure) => TmuxResolution {
            context,
            outcome: TmuxOutcome::Unavailable(failure),
        },
    }
}

/// Build the `display-message` specification for a single tmux format.
fn field_spec(format: &str) -> CommandSpec {
    CommandSpec::new("tmux").args(["display-message", "-p", format])
}

/// Query one tmux field, returning its trimmed value.
///
/// Each call's entire trimmed stdout *is* the field value, so no separator
/// character has to be reserved and no byte content can misalign the result.
fn query_field(runner: &dyn CommandRunner, field: TmuxField) -> Result<String, TmuxProbeFailure> {
    runner
        .run(&field_spec(field.format()))
        .map(|output| output.stdout.trim().to_owned())
        .map_err(|source| TmuxProbeFailure::CommandFailed { field, source })
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
fn query_fields(runner: &dyn CommandRunner) -> Result<Vec<(TmuxField, String)>, TmuxProbeFailure> {
    TMUX_FIELDS
        .into_iter()
        .map(|field| query_field(runner, field).map(|value| (field, value)))
        .collect()
}

/// Fill each unset field from the queried values, recording empty answers.
fn merge_fields(mut context: TmuxContext, values: Vec<(TmuxField, String)>) -> TmuxResolution {
    let mut malformed = Vec::new();
    for (field, value) in values {
        let slot = context.slot_mut(field);
        if slot.is_some() {
            continue;
        }
        if value.is_empty() {
            malformed.push(TmuxProbeFailure::EmptyValue { field });
            continue;
        }
        *slot = Some(value);
    }

    TmuxResolution {
        context,
        outcome: TmuxOutcome::Queried { malformed },
    }
}

#[cfg(test)]
mod tests;
