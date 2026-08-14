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
    /// ```rust,ignore
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
    /// ```rust,ignore
    /// use dbar::command::RealCommandRunner;
    /// use dbar::github::GhCliClient;
    ///
    /// let runner = RealCommandRunner::default();
    /// let client = GhCliClient::new(&runner);
    /// # let _ = client;
    /// ```
    pub fn new(runner: &'a dyn CommandRunner) -> Self {
        Self { runner }
    }
}

impl GitHubClient for GhCliClient<'_> {
    fn pr_number(
        &self,
        project_dir: &Utf8Path,
        _branch: &str,
    ) -> Result<Option<PrNumber>, GitHubError> {
        let output = self
            .runner
            .run(
                &CommandSpec::new("gh")
                    .args(["pr", "view", "--json", "number", "--jq", ".number"])
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
    /// ```rust,ignore
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

/// Errors returned by GitHub client implementations.
#[derive(Debug, Error)]
pub enum GitHubError {
    /// The `gh` CLI command failed.
    #[error("GitHub CLI command failed")]
    Command(#[from] CommandError),
}

#[cfg(test)]
mod tests {
    //! Tests for `gh` PR parsing, failure propagation, and the mock client.
    use super::*;
    use crate::command::{CommandError, CommandOutput, CommandRunner};
    use rstest::rstest;
    use std::cell::RefCell;

    /// Records the spec it was given and returns a canned result.
    struct StubRunner {
        result: Result<String, CommandError>,
        seen: RefCell<Option<CommandSpec>>,
    }

    impl StubRunner {
        fn ok(stdout: &str) -> Self {
            Self {
                result: Ok(stdout.to_owned()),
                seen: RefCell::new(None),
            }
        }

        fn failing() -> Self {
            Self {
                result: Err(CommandError::NonZero {
                    status: Some(1),
                    stderr: "no pull requests found".to_owned(),
                }),
                seen: RefCell::new(None),
            }
        }
    }

    impl CommandRunner for StubRunner {
        fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, CommandError> {
            *self.seen.borrow_mut() = Some(spec.clone());
            match &self.result {
                Ok(stdout) => Ok(CommandOutput {
                    stdout: stdout.clone(),
                }),
                Err(CommandError::NonZero { status, stderr }) => Err(CommandError::NonZero {
                    status: *status,
                    stderr: stderr.clone(),
                }),
                Err(_) => Err(CommandError::Timeout {
                    timeout: GH_TIMEOUT,
                }),
            }
        }
    }

    #[rstest]
    #[case::plain("42", "42")]
    #[case::trailing_newline("42\n", "42")]
    #[case::surrounding_space("  7  ", "7")]
    fn pr_number_parses_gh_output(#[case] stdout: &str, #[case] expected: &str) {
        let runner = StubRunner::ok(stdout);
        let client = GhCliClient::new(&runner);
        let pr = client
            .pr_number(Utf8Path::new("/projects/demo"), "main")
            .expect("lookup succeeds");
        assert_eq!(pr.map(|value| value.to_string()).as_deref(), Some(expected));
    }

    #[rstest]
    #[case::empty("")]
    #[case::whitespace_only("   \n")]
    fn pr_number_reports_no_pr_for_empty_output(#[case] stdout: &str) {
        let runner = StubRunner::ok(stdout);
        let client = GhCliClient::new(&runner);
        let pr = client
            .pr_number(Utf8Path::new("/projects/demo"), "main")
            .expect("lookup succeeds");
        assert!(pr.is_none(), "empty gh output means no open PR");
    }

    #[rstest]
    fn pr_number_propagates_command_failure() {
        let runner = StubRunner::failing();
        let client = GhCliClient::new(&runner);
        let err = client
            .pr_number(Utf8Path::new("/projects/demo"), "main")
            .expect_err("command failure must propagate");
        let GitHubError::Command(CommandError::NonZero { status, stderr }) = err else {
            panic!("expected a propagated non-zero command error");
        };
        assert_eq!(status, Some(1));
        assert_eq!(stderr, "no pull requests found");
    }

    #[rstest]
    fn pr_number_builds_the_expected_command_spec() {
        let runner = StubRunner::ok("1");
        let client = GhCliClient::new(&runner);
        let project_dir = Utf8Path::new("/projects/demo");
        let _ = client.pr_number(project_dir, "main").expect("lookup");

        let seen = runner.seen.borrow();
        let spec = seen.as_ref().expect("the runner was invoked");
        // The working directory scopes `gh` to the right repository, and the
        // timeout keeps a stalled network call off the status path.
        let expected = CommandSpec::new("gh")
            .args(["pr", "view", "--json", "number", "--jq", ".number"])
            .cwd(project_dir.to_path_buf())
            .timeout(GH_TIMEOUT)
            .max_output_bytes(GH_MAX_OUTPUT_BYTES);
        assert_eq!(*spec, expected);
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
            .pr_number(Utf8Path::new("/projects/demo"), "main")
            .expect("the mock never fails");
        assert_eq!(pr.map(|value| value.to_string()).as_deref(), expected);
    }
}
