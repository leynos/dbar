//! GitHub pull request lookup helpers.

use std::fmt;
use std::time::Duration;

use camino::Utf8Path;
use thiserror::Error;

use crate::command::{CommandError, CommandRunner, CommandSpec};
use crate::types::PrNumber;

/// Network probes should fail fast so the tmux status line stays responsive.
const GH_TIMEOUT: Duration = Duration::from_secs(5);

/// A PR number is a handful of bytes, so cap the reply well below the global
/// ceiling. A `gh` that floods stdout is terminated instead of being buffered,
/// and the oversized reply degrades to the branch-derived fallback rather than
/// stalling the status line.
const GH_MAX_OUTPUT_BYTES: usize = 64 * 1024;

/// GitHub client interface used for PR lookup.
pub trait GitHubClient {
    /// Resolve a PR number for the given project directory and branch.
    ///
    /// # Examples
    ///
    /// ```text
    /// use camino::Utf8Path;
    ///
    /// let client = MockGitHubClient::new("42");
    /// let pr = client.pr_number(Utf8Path::new("."), "main")?;
    /// assert!(pr.is_some());
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying GitHub client fails.
    fn pr_number(
        &self,
        project_dir: &Utf8Path,
        branch: &str,
    ) -> Result<Option<PrNumber>, GitHubError>;
}

/// Real GitHub client backed by the `gh` CLI.
pub struct GhCliClient<'a> {
    runner: &'a dyn CommandRunner,
}

impl<'a> GhCliClient<'a> {
    /// Construct a CLI-backed GitHub client.
    ///
    /// # Examples
    ///
    /// ```text
    /// // `runner` is any `CommandRunner`; a real one invokes the `gh`
    /// // binary, so tests inject `MockCommandRunner`.
    /// let client = GhCliClient::new(runner);
    /// ```
    pub fn new(runner: &'a dyn CommandRunner) -> Self {
        Self { runner }
    }
}

/// Build the `gh pr list` arguments that resolve the open PR for `branch`.
///
/// `--head=<branch>` keeps a dash-prefixed branch name attached to its flag:
/// passed as a separate argument it would be parsed as a flag of its own, and a
/// `--` separator cannot protect a flag *value*. The jq expression collapses an
/// empty result array to an empty string, so "no open PR" reaches the caller as
/// empty stdout rather than as a command failure.
fn pr_list_args(branch: &str) -> [String; 11] {
    [
        "pr".to_owned(),
        "list".to_owned(),
        format!("--head={branch}"),
        "--state".to_owned(),
        "open".to_owned(),
        "--limit".to_owned(),
        "1".to_owned(),
        "--json".to_owned(),
        "number".to_owned(),
        "--jq".to_owned(),
        r#".[0].number // """#.to_owned(),
    ]
}

impl GitHubClient for GhCliClient<'_> {
    fn pr_number(
        &self,
        project_dir: &Utf8Path,
        branch: &str,
    ) -> Result<Option<PrNumber>, GitHubError> {
        // `gh pr view <branch>` exits non-zero when the branch has no open PR,
        // which would turn "no PR" into a command failure and defeat the
        // negative cache. `gh pr list` with an exporter (`--json`) suppresses
        // the no-results error and prints an empty array instead, so the absent
        // case arrives as empty stdout and exit status zero.
        let output = self
            .runner
            .run(
                &CommandSpec::new("gh")
                    .args(pr_list_args(branch))
                    .cwd(project_dir.to_path_buf())
                    .timeout(GH_TIMEOUT)
                    .max_output_bytes(GH_MAX_OUTPUT_BYTES),
            )
            .map_err(|error| GitHubError::Command(CommandFailure::from(&error)))?
            .stdout;
        let value = output.trim();
        if value.is_empty() {
            Ok(None)
        } else {
            Ok(Some(PrNumber::new(value.to_owned())))
        }
    }
}

/// Mock GitHub client with a fixed PR value.
#[derive(Debug, Clone)]
pub struct MockGitHubClient {
    pr_number: Option<PrNumber>,
}

impl MockGitHubClient {
    /// Build a mock client with the provided PR number.
    ///
    /// # Examples
    ///
    /// ```text
    /// let client = MockGitHubClient::new("7");
    /// ```
    pub fn new(value: &str) -> Self {
        let trimmed = value.trim();
        let pr_number = if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("none") {
            None
        } else {
            Some(PrNumber::new(trimmed.to_owned()))
        };
        Self { pr_number }
    }
}

impl GitHubClient for MockGitHubClient {
    fn pr_number(
        &self,
        _project_dir: &Utf8Path,
        _branch: &str,
    ) -> Result<Option<PrNumber>, GitHubError> {
        Ok(self.pr_number.clone())
    }
}

/// A `gh` failure reduced to the facts that are safe to render.
///
/// [`CommandError::NonZero`] carries `gh`'s stderr verbatim, and `gh` prints
/// authentication diagnostics there — including remote URLs that may embed a
/// token, as in `https://x-access-token:<token>@github.com/...`. Rather than
/// wrap the whole [`CommandError`] and rely on every present and future
/// formatting site choosing `Display` over `Debug`, the stderr is discarded
/// where the error is built: this type is constructed from the category and
/// the exit status alone, so there is no secret left in the value for a
/// `{:?}`, an `unwrap`, or a `source()` walk to expose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandFailure {
    /// The process could not be spawned or read, with the I/O category that
    /// explains why. The underlying [`std::io::Error`] is dropped because its
    /// message can name the path it failed on.
    NotRun(std::io::ErrorKind),
    /// The process exited with this non-zero status code.
    ExitStatus(i32),
    /// The process was terminated by a signal, so it reported no status code.
    Signalled,
    /// The process ran past its timeout and was terminated.
    TimedOut(Duration),
    /// A stream exceeded its byte ceiling and the process was terminated.
    OutputTooLarge {
        /// The configured byte ceiling that was exceeded.
        limit: usize,
        /// Which stream exceeded the ceiling.
        stream: &'static str,
    },
}

impl From<&CommandError> for CommandFailure {
    fn from(error: &CommandError) -> Self {
        match error {
            CommandError::Io(io_error) => Self::NotRun(io_error.kind()),
            CommandError::NonZero {
                status: Some(code), ..
            } => Self::ExitStatus(*code),
            CommandError::NonZero { status: None, .. } => Self::Signalled,
            CommandError::Timeout { timeout } => Self::TimedOut(*timeout),
            CommandError::OutputTooLarge { limit, stream } => Self::OutputTooLarge {
                limit: *limit,
                stream,
            },
        }
    }
}

impl fmt::Display for CommandFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRun(kind) => write!(f, "the process could not be run ({kind})"),
            Self::ExitStatus(code) => write!(f, "exit status {code}"),
            Self::Signalled => f.write_str("terminated by a signal"),
            Self::TimedOut(timeout) => write!(f, "timed out after {timeout:?}"),
            Self::OutputTooLarge { limit, stream } => {
                write!(f, "{stream} exceeded the {limit}-byte output limit")
            }
        }
    }
}

/// Errors returned by GitHub client implementations.
///
/// Deliberately carries no `source`: the only thing worth reporting about a
/// failed `gh` invocation is its [`CommandFailure`] category, and chaining the
/// original [`CommandError`] would put its captured stderr back within reach
/// of `Debug` and [`std::error::Error::source`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum GitHubError {
    /// The `gh` CLI command failed.
    #[error("GitHub CLI command failed: {0}")]
    Command(CommandFailure),
}

#[cfg(test)]
mod tests;
