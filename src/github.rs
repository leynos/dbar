//! GitHub pull request lookup helpers.

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
    /// use dbar::github::{GitHubClient, MockGitHubClient};
    ///
    /// let client = MockGitHubClient::new("42");
    /// let pr = client.pr_number(Utf8Path::new("."), "main")?;
    /// assert!(pr.is_some());
    /// # Ok::<(), dbar::github::GitHubError>(())
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
            .map_err(GitHubError::Command)?
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
    /// use dbar::github::MockGitHubClient;
    ///
    /// let client = MockGitHubClient::new("7");
    /// # let _ = client;
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

/// Summarize a command failure without echoing the child's captured stderr.
///
/// `CommandError::NonZero` carries `gh`'s stderr verbatim, and `gh` prints
/// authentication diagnostics there — including remote URLs that may embed a
/// token, as in `https://x-access-token:<token>@github.com/...`. This message
/// reaches the tmux status line and any log that renders it, so only the
/// failure category and the exit status are exposed; the full text stays
/// reachable through the error's `source`.
///
/// # Examples
///
/// ```text
/// use std::time::Duration;
/// use dbar::command::CommandError;
///
/// let summary = command_failure_category(&CommandError::Timeout {
///     timeout: Duration::from_secs(5),
/// });
/// assert!(summary.starts_with("timed out"));
/// ```
fn command_failure_category(error: &CommandError) -> String {
    match error {
        CommandError::Io(_) => "the process could not be run".to_owned(),
        CommandError::NonZero {
            status: Some(code), ..
        } => format!("exit status {code}"),
        CommandError::NonZero { status: None, .. } => "terminated by a signal".to_owned(),
        CommandError::Timeout { timeout } => format!("timed out after {timeout:?}"),
        CommandError::OutputTooLarge { limit, stream } => {
            format!("{stream} exceeded the {limit}-byte output limit")
        }
    }
}

/// Errors returned by GitHub client implementations.
#[derive(Debug, Error)]
pub enum GitHubError {
    /// The `gh` CLI command failed.
    #[error("GitHub CLI command failed: {}", command_failure_category(.0))]
    Command(#[from] CommandError),
}

#[cfg(test)]
mod tests {
    //! Tests for `gh` PR parsing, failure propagation, and the mock client.
    use super::*;
    use crate::command::{CommandError, CommandOutput, MockCommandRunner};
    use mockall::predicate::eq;
    use rstest::rstest;

    /// The project directory every lookup under test is rooted at.
    const PROJECT_DIR: &str = "/projects/demo";

    /// The spec `pr_number` is expected to build for `branch`.
    fn expected_spec(project_dir: &Utf8Path, branch: &str) -> CommandSpec {
        // The working directory scopes `gh` to the right repository, `--head`
        // overrides the current checkout, `--json` suppresses the no-results
        // error so an absent PR is not a failure, and the timeout keeps a
        // stalled network call off the status path. Spelled out rather than
        // reusing `pr_list_args`, so a change to the real command has to be
        // restated here deliberately.
        CommandSpec::new("gh")
            .args([
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
            ])
            .cwd(project_dir.to_path_buf())
            .timeout(GH_TIMEOUT)
            .max_output_bytes(GH_MAX_OUTPUT_BYTES)
    }

    /// A runner that answers the one expected lookup with `stdout`.
    fn runner_answering(stdout: &str) -> MockCommandRunner {
        let answer = stdout.to_owned();
        let mut runner = MockCommandRunner::new();
        runner
            .expect_run()
            .with(eq(expected_spec(Utf8Path::new(PROJECT_DIR), "main")))
            .times(1)
            .returning(move |_| {
                Ok(CommandOutput {
                    stdout: answer.clone(),
                })
            });
        runner
    }

    #[rstest]
    #[case::plain("42", "42")]
    #[case::trailing_newline("42\n", "42")]
    #[case::surrounding_space("  7  ", "7")]
    fn pr_number_parses_gh_output(#[case] stdout: &str, #[case] expected: &str) {
        let runner = runner_answering(stdout);
        let client = GhCliClient::new(&runner);
        let pr = client
            .pr_number(Utf8Path::new(PROJECT_DIR), "main")
            .expect("lookup succeeds");
        assert_eq!(pr.map(|value| value.to_string()).as_deref(), Some(expected));
    }

    #[rstest]
    #[case::empty("")]
    #[case::whitespace_only("   \n")]
    fn pr_number_reports_no_pr_for_empty_output(#[case] stdout: &str) {
        let runner = runner_answering(stdout);
        let client = GhCliClient::new(&runner);
        let pr = client
            .pr_number(Utf8Path::new(PROJECT_DIR), "main")
            .expect("lookup succeeds");
        assert!(pr.is_none(), "empty gh output means no open PR");
    }

    #[rstest]
    fn pr_number_propagates_command_failure() {
        let mut runner = MockCommandRunner::new();
        runner
            .expect_run()
            .with(eq(expected_spec(Utf8Path::new(PROJECT_DIR), "main")))
            .times(1)
            .returning(|_| {
                Err(CommandError::NonZero {
                    status: Some(1),
                    stderr: "could not determine the current repository".to_owned(),
                })
            });
        let client = GhCliClient::new(&runner);
        let err = client
            .pr_number(Utf8Path::new(PROJECT_DIR), "main")
            .expect_err("command failure must propagate");
        let GitHubError::Command(CommandError::NonZero { status, stderr }) = err else {
            panic!("expected a propagated non-zero command error");
        };
        assert_eq!(status, Some(1));
        assert_eq!(stderr, "could not determine the current repository");
    }

    #[rstest]
    #[case::plain_branch("feature/login")]
    // A dash-prefixed name must reach `gh` as the value of `--head` rather than
    // being parsed as a flag, which is what the `--head=<branch>` form
    // guarantees.
    #[case::dash_prefixed_branch("-weird-branch")]
    fn pr_number_builds_the_expected_command_spec(#[case] branch: &str) {
        let project_dir = Utf8Path::new(PROJECT_DIR);
        // The expectation itself is the assertion: any other spec fails to
        // match and the mock panics, and `times(1)` fails the test at drop if
        // the lookup never ran.
        let mut runner = MockCommandRunner::new();
        runner
            .expect_run()
            .with(eq(expected_spec(project_dir, branch)))
            .times(1)
            .returning(|_| {
                Ok(CommandOutput {
                    stdout: "1".to_owned(),
                })
            });
        let client = GhCliClient::new(&runner);
        let _ = client.pr_number(project_dir, branch).expect("lookup");
    }

    #[rstest]
    #[case::non_zero(
        CommandError::NonZero {
            status: Some(1),
            stderr: "gh: token ghp_supersecret is invalid".to_owned(),
        },
        "GitHub CLI command failed: exit status 1",
    )]
    #[case::signalled(
        CommandError::NonZero { status: None, stderr: "killed".to_owned() },
        "GitHub CLI command failed: terminated by a signal",
    )]
    #[case::timeout(
        CommandError::Timeout { timeout: GH_TIMEOUT },
        "GitHub CLI command failed: timed out after 5s",
    )]
    #[case::too_large(
        CommandError::OutputTooLarge { limit: 8, stream: "stdout" },
        "GitHub CLI command failed: stdout exceeded the 8-byte output limit",
    )]
    fn command_error_displays_the_failure_category(
        #[case] source: CommandError,
        #[case] expected: &str,
    ) {
        let err = GitHubError::from(source);
        assert_eq!(err.to_string(), expected);
        assert!(
            !err.to_string().contains("ghp_"),
            "captured stderr must not reach the rendered message"
        );
    }

    #[rstest]
    #[case::number("42", Some("42"))]
    #[case::padded_number("  42  ", Some("42"))]
    #[case::empty("", None)]
    #[case::whitespace("   ", None)]
    #[case::none_lowercase("none", None)]
    #[case::none_uppercase("NONE", None)]
    #[case::none_mixed("NoNe", None)]
    fn mock_client_maps_its_configured_value(
        #[case] configured: &str,
        #[case] expected: Option<&str>,
    ) {
        let client = MockGitHubClient::new(configured);
        let pr = client
            .pr_number(Utf8Path::new(PROJECT_DIR), "main")
            .expect("the mock never fails");
        assert_eq!(pr.map(|value| value.to_string()).as_deref(), expected);
    }
}
