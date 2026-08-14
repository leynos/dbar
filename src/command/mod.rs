//! Command execution helpers for git and tmux probes.

use std::io::{self, Read};
use std::process::{Child, Command, Stdio};
use std::thread::JoinHandle;
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

/// Take a piped handle off the child, mapping the impossible `None` to an error.
fn take_pipe<T>(pipe: Option<T>) -> Result<T, CommandError> {
    pipe.ok_or_else(|| CommandError::Io(io::Error::other("child pipe unavailable")))
}

/// What a bounded reader thread observed on its stream.
enum ReadOutcome {
    /// The stream ended within its ceiling, yielding these bytes.
    Bytes(Vec<u8>),
    /// The stream exceeded its ceiling; the payload was discarded.
    TooLarge,
}

/// Drain a child pipe on its own thread so the child never blocks on a full
/// pipe buffer while the parent is waiting for it to exit.
///
/// The read is bounded at `limit` bytes. One byte beyond the ceiling is read so
/// that overrunning the limit is distinguishable from exactly reaching it; if
/// that extra byte materializes the buffer is dropped and the child's process
/// group is killed, because there is no point letting a runaway producer keep
/// writing into a capture that has already been abandoned.
fn spawn_reader(
    mut pipe: impl Read + Send + 'static,
    limit: usize,
    pid: u32,
) -> JoinHandle<io::Result<ReadOutcome>> {
    std::thread::spawn(move || {
        let ceiling = u64::try_from(limit)
            .map_err(|_| io::Error::other("output limit does not fit in a byte count"))?;
        let mut buffer = Vec::new();
        pipe.by_ref().take(ceiling + 1).read_to_end(&mut buffer)?;
        if buffer.len() > limit {
            // Release the oversized payload before doing anything else; it must
            // not be retained or handed back to the caller.
            drop(buffer);
            // Best effort: the caller's cleanup paths are the backstop, so a
            // failure here is not worth reporting over the size violation.
            drop(signal_process_group(pid));
            return Ok(ReadOutcome::TooLarge);
        }
        Ok(ReadOutcome::Bytes(buffer))
    })
}

/// Collect a reader thread's outcome.
fn join_reader(handle: JoinHandle<io::Result<ReadOutcome>>) -> Result<ReadOutcome, CommandError> {
    let outcome = handle
        .join()
        .map_err(|_| io::Error::other("output reader thread panicked"))??;
    Ok(outcome)
}

/// Unwrap a reader outcome, reporting an overrun against the named stream.
fn require_within_limit(
    outcome: ReadOutcome,
    limit: usize,
    stream: &'static str,
) -> Result<Vec<u8>, CommandError> {
    match outcome {
        ReadOutcome::Bytes(bytes) => Ok(bytes),
        ReadOutcome::TooLarge => Err(CommandError::OutputTooLarge { limit, stream }),
    }
}

/// Put the child in its own process group so its descendants can be signalled
/// as a unit.
#[cfg(unix)]
fn use_own_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt as _;
    command.process_group(0);
}

#[cfg(not(unix))]
fn use_own_process_group(_command: &mut Command) {}

/// Terminate the child and every descendant sharing its process group.
///
/// Signalling only the direct child would leave a backgrounded grandchild
/// holding the inherited pipe write end open, so the reader threads would never
/// observe EOF and the timeout would block instead of returning.
///
/// The pid is taken raw rather than as a `&Child` so that a reader thread,
/// which does not own the child handle, can terminate the group too.
#[cfg(unix)]
fn signal_process_group(child_pid: u32) -> Result<(), CommandError> {
    let raw = i32::try_from(child_pid)
        .map_err(|_| io::Error::other("child pid does not fit in a process id"))?;
    let pid = rustix::process::Pid::from_raw(raw)
        .ok_or_else(|| io::Error::other("child pid is not a valid process id"))?;
    match rustix::process::kill_process_group(pid, rustix::process::Signal::KILL) {
        // An empty group means every descendant has already exited, which is
        // exactly the state the caller wants; treat it as success.
        Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
        Err(err) => Err(CommandError::Io(io::Error::from(err))),
    }
}

#[cfg(not(unix))]
fn signal_process_group(_child_pid: u32) -> Result<(), CommandError> {
    Ok(())
}

/// Terminate the child and every descendant, then reap the direct child.
fn terminate_child_tree(child: &mut Child) -> Result<(), CommandError> {
    #[cfg(not(unix))]
    child.kill()?;
    signal_process_group(child.id())?;
    child.wait()?;
    Ok(())
}

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
        use_own_process_group(&mut command);
        let timeout = spec.timeout.unwrap_or(DEFAULT_TIMEOUT);
        let max_output = spec.max_output_bytes.unwrap_or(DEFAULT_MAX_OUTPUT_BYTES);
        let mut child = command.spawn()?;
        let pid = child.id();

        // Drain both pipes concurrently with the wait. A child that writes more
        // than the OS pipe buffer (64 KiB on Linux) blocks in `write` until the
        // parent reads, which would otherwise be misreported as a timeout.
        let stdout_reader = spawn_reader(take_pipe(child.stdout.take())?, max_output, pid);
        let stderr_reader = spawn_reader(take_pipe(child.stderr.take())?, max_output, pid);

        let Some(status) = child.wait_timeout(timeout)? else {
            // Kill the whole group: any descendant still holding the inherited
            // pipe write end would otherwise keep the readers from seeing EOF.
            terminate_child_tree(&mut child)?;
            // Every writer is now gone, so the readers finish promptly; join
            // them to avoid leaking threads, but keep the timeout as the
            // reported failure.
            drop(join_reader(stdout_reader));
            drop(join_reader(stderr_reader));
            return Err(CommandError::Timeout { timeout });
        };

        // The direct child has exited, but a descendant it backgrounded may
        // still hold the inherited pipe write ends. Without releasing the group
        // the reader joins below would block indefinitely, and this path has no
        // timeout to fall back on.
        signal_process_group(pid)?;

        let stdout_outcome = join_reader(stdout_reader)?;
        let stderr_outcome = join_reader(stderr_reader)?;
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
