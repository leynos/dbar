//! GitHub pull request lookup helpers.

use camino::Utf8Path;
use thiserror::Error;

use crate::command::{CommandError, CommandRunner, CommandSpec};
use crate::types::PrNumber;

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
                    .cwd(project_dir.to_path_buf()),
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
