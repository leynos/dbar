//! Command execution helpers for git and tmux probes.
//!
//! # Platform support
//!
//! dbar is a Unix-only crate, and this module is where that is enforced for the
//! whole build. The claim is not merely that tmux — the only thing dbar exists
//! to drive — is a Unix program. It is that dbar's two safety properties are
//! both implemented with POSIX primitives that have no portable equivalent
//! here:
//!
//! - the probe timeout below depends on placing the child in its own process
//!   group and signalling that group, because killing only the direct child
//!   leaves a backgrounded grandchild holding the inherited pipe write end and
//!   the reader threads never see EOF;
//! - the install transaction in `crate::install::fs` depends on `flock`.
//!
//! Both were previously stubbed out on non-Unix targets so that the crate would
//! notionally compile there, which meant a non-Unix build silently ran with no
//! descendant kill and no install lock while the surrounding code went on
//! claiming to be transactional. A build that cannot uphold what it promises is
//! worse than one that refuses to exist, so it now refuses. `rustix`, the crate
//! already used for both primitives, supports only Winsock on Windows, so
//! adding real support would mean a new dependency and a Windows story nobody
//! runs; that decision has not been taken.
#[cfg(not(unix))]
compile_error!(
    "dbar supports Unix targets only: its probe timeout needs POSIX process \
     groups and its install transaction needs flock, and stubbing either out \
     would silently drop a guarantee the code claims to provide"
);

use std::process::Command;
use std::time::Duration;

use camino::Utf8PathBuf;
use thiserror::Error;

mod child;

use child::{ChildSession, capture_output, require_within_limit, use_own_process_group};

/// Default wall-clock ceiling applied when a spec sets no explicit timeout.
///
/// Probes shell out to `git`, `gh`, and `tmux`; a hung child (for example a
/// `gh` call stalled on the network) would otherwise freeze the tmux status
/// line indefinitely.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// Default ceiling on the bytes captured from each of the child's streams.
///
/// The realistic large producer is `git status --porcelain` in a very large
/// repository, which emits one line per changed path and can therefore grow
/// without any natural bound. Buffering that in full would let a single probe
/// balloon the status line's memory for no benefit, because
/// `git_worktree_status` only inspects the first two characters of each line.
/// Four mebibytes is far beyond any status output worth summarizing.
const DEFAULT_MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
/// A command specification used by probes.
pub struct CommandSpec {
    program: String,
    args: Vec<String>,
    cwd: Option<Utf8PathBuf>,
    timeout: Option<Duration>,
    max_output_bytes: Option<usize>,
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
            max_output_bytes: None,
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

    /// Override the per-stream output ceiling for this command.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use dbar::command::CommandSpec;
    ///
    /// let spec = CommandSpec::new("git").max_output_bytes(1024);
    /// ```
    pub const fn max_output_bytes(mut self, max_output_bytes: usize) -> Self {
        self.max_output_bytes = Some(max_output_bytes);
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
    /// A stream exceeded its configured byte ceiling and was terminated.
    #[error("{stream} exceeded the {limit}-byte output limit and was terminated")]
    OutputTooLarge {
        /// The configured byte ceiling that was exceeded.
        limit: usize,
        /// Which stream exceeded the ceiling.
        stream: &'static str,
    },
}

/// Executes external commands for probes.
///
/// Test builds also gain a `MockCommandRunner` generated by `mockall`, which is
/// the approved way to double this seam; see the developers' guide.
#[cfg_attr(test, mockall::automock)]
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
        command.args(&spec.args);
        capture_output(&mut command);
        if let Some(cwd) = &spec.cwd {
            command.current_dir(cwd.as_std_path());
        }
        use_own_process_group(&mut command);
        let timeout = spec.timeout.unwrap_or(DEFAULT_TIMEOUT);
        let max_output = spec.max_output_bytes.unwrap_or(DEFAULT_MAX_OUTPUT_BYTES);
        // Past this point every exit path — including each `?` below — runs
        // `ChildSession::drop`, which releases the process group, reaps the
        // child, and joins whichever readers are still outstanding.
        let mut session = ChildSession::new(command.spawn()?);
        session.start_readers(max_output)?;

        let Some(status) = session.wait_for(timeout)? else {
            // Kill the whole group: any descendant still holding the inherited
            // pipe write end would otherwise keep the readers from seeing EOF.
            // Every writer is then gone, so the guard's joins finish promptly,
            // and the timeout stays the reported failure.
            session.release_process_group()?;
            session.reap()?;
            return Err(CommandError::Timeout { timeout });
        };

        // The direct child has exited, but a descendant it backgrounded may
        // still hold the inherited pipe write ends. Without releasing the group
        // the reader joins below would block indefinitely, and this path has no
        // timeout to fall back on.
        session.release_process_group()?;

        let (stdout_outcome, stderr_outcome) = session.join_readers()?;
        // An overrun kills the child, so its exit status reflects the signal
        // rather than anything the command decided; report the size violation
        // before consulting the status at all.
        let stdout_bytes = require_within_limit(stdout_outcome, max_output, "stdout")?;
        let stderr_bytes = require_within_limit(stderr_outcome, max_output, "stderr")?;
        if !status.success() {
            let stderr = String::from_utf8_lossy(&stderr_bytes).trim().to_owned();
            return Err(CommandError::NonZero {
                status: status.code(),
                stderr,
            });
        }
        let stdout = String::from_utf8_lossy(&stdout_bytes).trim().to_owned();
        Ok(CommandOutput { stdout })
    }
}

#[cfg(test)]
mod tests;
