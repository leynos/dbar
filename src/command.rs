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

/// Take a piped handle off the child, mapping the impossible `None` to an error.
fn take_pipe<T>(pipe: Option<T>) -> Result<T, CommandError> {
    pipe.ok_or_else(|| CommandError::Io(io::Error::other("child pipe unavailable")))
}

/// Drain a child pipe on its own thread so the child never blocks on a full
/// pipe buffer while the parent is waiting for it to exit.
fn spawn_reader(mut pipe: impl Read + Send + 'static) -> JoinHandle<io::Result<Vec<u8>>> {
    std::thread::spawn(move || {
        let mut buffer = Vec::new();
        pipe.read_to_end(&mut buffer)?;
        Ok(buffer)
    })
}

/// Collect a reader thread's buffered bytes.
fn join_reader(handle: JoinHandle<io::Result<Vec<u8>>>) -> Result<Vec<u8>, CommandError> {
    let bytes = handle
        .join()
        .map_err(|_| io::Error::other("output reader thread panicked"))??;
    Ok(bytes)
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
#[cfg(unix)]
fn signal_process_group(child: &Child) -> Result<(), CommandError> {
    let raw = i32::try_from(child.id())
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
fn signal_process_group(_child: &Child) -> Result<(), CommandError> {
    Ok(())
}

/// Terminate the child and every descendant, then reap the direct child.
fn terminate_child_tree(child: &mut Child) -> Result<(), CommandError> {
    #[cfg(not(unix))]
    child.kill()?;
    signal_process_group(child)?;
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
        let mut child = command.spawn()?;

        // Drain both pipes concurrently with the wait. A child that writes more
        // than the OS pipe buffer (64 KiB on Linux) blocks in `write` until the
        // parent reads, which would otherwise be misreported as a timeout.
        let stdout_reader = spawn_reader(take_pipe(child.stdout.take())?);
        let stderr_reader = spawn_reader(take_pipe(child.stderr.take())?);

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
        signal_process_group(&child)?;

        let stdout_bytes = join_reader(stdout_reader)?;
        let stderr_bytes = join_reader(stderr_reader)?;
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
mod tests {
    //! Tests for real command execution, timeout handling, and error mapping.
    use super::*;
    use rstest::rstest;
    use std::time::Instant;

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
    fn run_captures_output_larger_than_the_pipe_buffer() {
        // The child writes far more than the 64 KiB pipe buffer before exiting.
        // Without concurrent draining it would block in `write` and be
        // misreported as a timeout.
        let runner = RealCommandRunner;
        let spec = CommandSpec::new("sh")
            .args(["-c", "head -c 200000 /dev/zero | tr '\\0' 'x'; exit 0"])
            .timeout(Duration::from_secs(30));
        let output = runner.run(&spec).expect("large output must not time out");
        assert_eq!(output.stdout.len(), 200_000);
    }

    #[rstest]
    fn run_returns_promptly_when_a_descendant_outlives_the_child() {
        // The direct child exits immediately, so the timeout path never runs,
        // but it leaves a backgrounded grandchild holding the inherited pipes.
        // Without releasing the process group the reader joins would block for
        // the grandchild's whole lifetime with no timeout to rescue them.
        let runner = RealCommandRunner;
        let spec = CommandSpec::new("sh")
            .args(["-c", "sleep 30 & exit 0"])
            .timeout(Duration::from_secs(20));

        let started = Instant::now();
        let output = runner.run(&spec).expect("child exits successfully");
        let elapsed = started.elapsed();

        assert!(output.stdout.is_empty());
        assert!(
            elapsed < Duration::from_secs(5),
            "run() must not wait on a surviving descendant, took {elapsed:?}"
        );
    }

    #[rstest]
    fn run_times_out_promptly_despite_a_pipe_inheriting_descendant() {
        // The direct child stalls past the timeout so the kill path runs, and
        // it first backgrounds a grandchild that inherits the stdout/stderr
        // pipes. Killing only the direct child would leave the grandchild
        // holding the write end, so the readers would never reach EOF and the
        // join would block instead of returning the timeout.
        let runner = RealCommandRunner;
        let spec = CommandSpec::new("sh")
            .args(["-c", "sleep 30 & sleep 5"])
            .timeout(Duration::from_millis(200));

        let started = Instant::now();
        let err = runner
            .run(&spec)
            .expect_err("stalled command must time out");
        let elapsed = started.elapsed();

        assert!(matches!(err, CommandError::Timeout { .. }));
        assert!(
            elapsed < Duration::from_secs(5),
            "timeout must return promptly, took {elapsed:?}"
        );
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
