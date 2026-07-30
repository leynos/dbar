//! Command execution helpers for git and tmux probes.

use std::process::{Command, Stdio};
use std::time::Duration;

use camino::Utf8PathBuf;
use thiserror::Error;
use wait_timeout::ChildExt;

/// Default wall-clock ceiling applied when a spec sets no explicit timeout.
///
/// Probes shell out to `git`, `gh`, and `tmux`; a hung child (for example a
/// `gh` call stalled on the network) would otherwise freeze the tmux status
/// line indefinitely.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
/// A command specification used by probes.
pub struct CommandSpec {
    program: String,
    args: Vec<String>,
    cwd: Option<Utf8PathBuf>,
    timeout: Option<Duration>,
}

impl CommandSpec {
    /// Create a new command specification.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use dbar::command::CommandSpec;
    ///
    /// let spec = CommandSpec::new("git");
    /// ```
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: None,
            timeout: None,
        }
    }

    /// Attach command arguments.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use dbar::command::CommandSpec;
    ///
    /// let spec = CommandSpec::new("git").args(["status", "--porcelain"]);
    /// ```
    pub fn args(mut self, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }

    /// Set the working directory for the command.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use camino::Utf8PathBuf;
    /// use dbar::command::CommandSpec;
    ///
    /// let spec = CommandSpec::new("git").cwd(Utf8PathBuf::from("."));
    /// ```
    pub fn cwd(mut self, cwd: Utf8PathBuf) -> Self {
        self.cwd = Some(cwd);
        self
    }

    /// Override the execution timeout for this command.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use std::time::Duration;
    /// use dbar::command::CommandSpec;
    ///
    /// let spec = CommandSpec::new("git").timeout(Duration::from_secs(2));
    /// ```
    pub const fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
}

#[derive(Debug, Clone)]
/// Captured command output.
pub struct CommandOutput {
    /// The stdout payload captured from the command.
    pub stdout: String,
}

#[derive(Debug, Error)]
/// Errors emitted while running commands.
pub enum CommandError {
    /// The process could not be spawned or read.
    #[error("failed to execute command")]
    Io(#[from] std::io::Error),
    /// The process exited with a non-zero status.
    #[error("command exited with status {status:?}: {stderr}")]
    NonZero {
        /// The exit status code, if available.
        status: Option<i32>,
        /// Collected stderr output.
        stderr: String,
    },
    /// The process ran longer than its timeout and was terminated.
    #[error("command timed out after {timeout:?}")]
    Timeout {
        /// The elapsed ceiling that was exceeded.
        timeout: Duration,
    },
}

/// Executes external commands for probes.
pub trait CommandRunner {
    /// Run the command and capture its output.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use dbar::command::{CommandRunner, CommandSpec, RealCommandRunner};
    ///
    /// let runner = RealCommandRunner::default();
    /// let spec = CommandSpec::new("true");
    /// let output = runner.run(&spec);
    /// assert!(output.is_ok());
    /// ```
    fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, CommandError>;
}

#[derive(Debug, Default)]
/// A command runner that executes real processes.
pub struct RealCommandRunner;

impl CommandRunner for RealCommandRunner {
    fn run(&self, spec: &CommandSpec) -> Result<CommandOutput, CommandError> {
        let mut command = Command::new(&spec.program);
        command
            .args(&spec.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(cwd) = &spec.cwd {
            command.current_dir(cwd.as_std_path());
        }
        let timeout = spec.timeout.unwrap_or(DEFAULT_TIMEOUT);
        let mut child = command.spawn()?;
        if child.wait_timeout(timeout)?.is_none() {
            // The probes emit small outputs, so a child that has not exited by
            // now is genuinely stalled: terminate it and report the timeout.
            child.kill()?;
            child.wait()?;
            return Err(CommandError::Timeout { timeout });
        }
        let output = child.wait_with_output()?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            return Err(CommandError::NonZero {
                status: output.status.code(),
                stderr,
            });
        }
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        Ok(CommandOutput { stdout })
    }
}

#[cfg(test)]
mod tests {
    //! Tests for real command execution, timeout handling, and error mapping.
    use super::*;
    use rstest::rstest;

    #[rstest]
    fn run_captures_stdout() {
        let runner = RealCommandRunner;
        let spec = CommandSpec::new("printf").args(["hello"]);
        let output = runner.run(&spec).expect("printf runs");
        assert_eq!(output.stdout, "hello");
    }

    #[rstest]
    fn run_reports_non_zero_status() {
        let runner = RealCommandRunner;
        let spec = CommandSpec::new("false");
        let err = runner.run(&spec).expect_err("false exits non-zero");
        assert!(matches!(err, CommandError::NonZero { .. }));
    }

    #[rstest]
    fn run_times_out_a_stalled_command() {
        let runner = RealCommandRunner;
        let spec = CommandSpec::new("sleep")
            .args(["5"])
            .timeout(Duration::from_millis(100));
        let err = runner.run(&spec).expect_err("sleep exceeds the timeout");
        assert!(matches!(err, CommandError::Timeout { .. }));
    }
}
