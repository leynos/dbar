//! The spawned child, its bounded reader threads, and their cleanup.
//!
//! Split out of `super` so that the guard and the pipe-draining machinery it
//! coordinates sit together, away from the command specification and the error
//! taxonomy. Everything here is Unix-only; see the module docs on `super` for
//! why the crate as a whole is.

use std::io::{self, Read};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use wait_timeout::ChildExt as _;

use super::CommandError;
use super::signal::{SignalClaim, Signaller};

/// Configure a command to capture both streams with no stdin of its own.
pub(super) fn capture_output(command: &mut Command) {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
}

/// Take a piped handle off the child, mapping the impossible `None` to an error.
fn take_pipe<T>(pipe: Option<T>) -> Result<T, CommandError> {
    pipe.ok_or_else(|| CommandError::Io(io::Error::other("child pipe unavailable")))
}

/// What a bounded reader thread observed on its stream.
pub(super) enum ReadOutcome {
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
///
/// That kill goes through the shared claim, so it cannot double up with the
/// session's own and cannot land once the session has reaped the child.
fn spawn_reader(
    mut pipe: impl Read + Send + 'static,
    limit: usize,
    claim: Arc<SignalClaim>,
) -> JoinHandle<io::Result<ReadOutcome>> {
    std::thread::spawn(move || {
        let ceiling = u64::try_from(limit)
            .map_err(|_| io::Error::other("output limit does not fit in a byte count"))?;
        let mut buffer = Vec::new();
        // Saturating rather than checked: at `u64::MAX` the sentinel byte is
        // meaningless, because no stream can overrun a ceiling that large, so
        // clamping reads everything the child produces. Wrapping instead would
        // take(0) and silently capture nothing.
        pipe.by_ref()
            .take(ceiling.saturating_add(1))
            .read_to_end(&mut buffer)?;
        if buffer.len() > limit {
            // Release the oversized payload before doing anything else; it must
            // not be retained or handed back to the caller.
            drop(buffer);
            // Best effort: the caller's cleanup paths are the backstop, so a
            // failure here is not worth reporting over the size violation.
            drop(claim.signal(Signaller::Reader));
            return Ok(ReadOutcome::TooLarge);
        }
        Ok(ReadOutcome::Bytes(buffer))
    })
}

#[cfg(test)]
thread_local! {
    /// Counts the readers joined on the current thread, so that a test can
    /// assert a dropped session collected its threads instead of abandoning
    /// them.
    ///
    /// Thread-local rather than global because every join runs on the thread
    /// that owns the session — the joining thread in `Drop` is the dropping
    /// thread — so per-thread counting needs no coordination between tests
    /// running concurrently.
    static READERS_JOINED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many readers this thread has joined so far.
#[cfg(test)]
pub(super) fn readers_joined() -> usize {
    READERS_JOINED.with(std::cell::Cell::get)
}

/// Collect a reader thread's outcome.
fn join_reader(handle: JoinHandle<io::Result<ReadOutcome>>) -> Result<ReadOutcome, CommandError> {
    let joined = handle.join();
    // Counted before the failure paths below, because a panicked reader has
    // still been collected rather than left running.
    #[cfg(test)]
    READERS_JOINED.with(|count| count.set(count.get().saturating_add(1)));
    let outcome = joined.map_err(|_| io::Error::other("output reader thread panicked"))??;
    Ok(outcome)
}

/// Unwrap a reader outcome, reporting an overrun against the named stream.
pub(super) fn require_within_limit(
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
pub(super) fn use_own_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt as _;
    command.process_group(0);
}

/// Join a reader that should have been started, reporting its absence rather
/// than panicking on the `None` that cannot occur.
fn join_started_reader(
    handle: Option<JoinHandle<io::Result<ReadOutcome>>>,
) -> Result<ReadOutcome, CommandError> {
    handle.map_or_else(
        || {
            Err(CommandError::Io(io::Error::other(
                "output reader not started",
            )))
        },
        join_reader,
    )
}

/// Owns a spawned child and its two reader threads for the whole of
/// [`RealCommandRunner::run`].
///
/// Cleanup lives in `Drop` rather than at each `return` because the hand-written
/// version only covered the exit paths somebody remembered. Every `?` between
/// the spawn and the joins — taking the pipes, `wait_timeout`, the group kill,
/// the joins themselves — used to return with the child unreaped and the reader
/// threads still blocked on pipes that nothing would ever close. That is the
/// same shape of leak already fixed twice here, once on the timeout path and
/// once on the success path; routing all of it through one guard means the next
/// error path added cannot reintroduce it.
pub(super) struct ChildSession {
    /// The direct child, retained so it can be reaped.
    child: Child,
    /// The right to kill the child's process group, shared with both reader
    /// threads so that only one of the three ever spends it.
    claim: Arc<SignalClaim>,
    /// Whether the direct child has been reaped, by `wait_timeout` or by
    /// [`Self::reap`].
    reaped: bool,
    /// The stdout reader, taken when it is joined.
    stdout_reader: Option<JoinHandle<io::Result<ReadOutcome>>>,
    /// The stderr reader, taken when it is joined.
    stderr_reader: Option<JoinHandle<io::Result<ReadOutcome>>>,
}

impl ChildSession {
    /// Take ownership of a freshly spawned child.
    pub(super) fn new(child: Child) -> Self {
        let claim = Arc::new(SignalClaim::new(child.id()));
        Self {
            child,
            claim,
            reaped: false,
            stdout_reader: None,
            stderr_reader: None,
        }
    }

    /// Take both pipes off the child and start a bounded reader for each.
    ///
    /// Draining concurrently with the wait is what stops a child that writes
    /// more than the OS pipe buffer (64 KiB on Linux) from blocking in `write`
    /// and being misreported as a timeout.
    pub(super) fn start_readers(&mut self, limit: usize) -> Result<(), CommandError> {
        let stdout = take_pipe(self.child.stdout.take())?;
        self.stdout_reader = Some(spawn_reader(stdout, limit, Arc::clone(&self.claim)));
        let stderr = take_pipe(self.child.stderr.take())?;
        self.stderr_reader = Some(spawn_reader(stderr, limit, Arc::clone(&self.claim)));
        Ok(())
    }

    /// Wait up to `timeout` for the child, recording the reap if it exited.
    ///
    /// The reap is recorded against the shared claim as soon as `wait_timeout`
    /// reports it, which is what stops a reader signalling a pid this process
    /// no longer owns.
    pub(super) fn wait_for(
        &mut self,
        timeout: Duration,
    ) -> Result<Option<ExitStatus>, CommandError> {
        let status = self.child.wait_timeout(timeout)?;
        if status.is_some() {
            self.mark_reaped();
        }
        Ok(status)
    }

    /// Note the reap locally and against the shared claim.
    fn mark_reaped(&mut self) {
        self.reaped = true;
        self.claim.mark_reaped();
    }

    /// Terminate the child and every descendant sharing its process group.
    ///
    /// Idempotent, and mutually exclusive with the readers' kill: whichever of
    /// the three spends the claim first is the only one to signal, so the
    /// explicit call on a failure path, the one in `Drop`, and an overrunning
    /// reader cannot combine into a second kill aimed at a recycled pid.
    pub(super) fn release_process_group(&mut self) -> Result<(), CommandError> {
        self.claim.signal(Signaller::Session)
    }

    /// Reap the direct child, at most once.
    pub(super) fn reap(&mut self) -> Result<(), CommandError> {
        if self.reaped {
            return Ok(());
        }
        // Recorded before the wait is checked: a wait that failed still leaves
        // a pid nobody here should be signalling.
        let waited = self.child.wait();
        self.mark_reaped();
        waited?;
        Ok(())
    }

    /// Kills a reader issued after the reap was recorded.
    #[cfg(test)]
    pub(super) fn post_reap_reader_signals(&self) -> usize {
        self.claim.post_reap_reader_signals()
    }

    /// Join both readers, collecting the second before reporting the first's
    /// failure so that neither thread is left behind by an early return.
    pub(super) fn join_readers(&mut self) -> Result<(ReadOutcome, ReadOutcome), CommandError> {
        let stdout = join_started_reader(self.stdout_reader.take());
        let stderr = join_started_reader(self.stderr_reader.take());
        Ok((stdout?, stderr?))
    }

    /// Join whichever readers are still outstanding, discarding their outcomes.
    fn join_outstanding_readers(&mut self) {
        for handle in [self.stdout_reader.take(), self.stderr_reader.take()]
            .into_iter()
            .flatten()
        {
            drop(join_reader(handle));
        }
    }
}

impl Drop for ChildSession {
    fn drop(&mut self) {
        // Order matters: a reader only reaches EOF once every process holding
        // the write end has gone, so the group is released first and joined
        // second.
        let released = self.release_process_group().is_ok();
        drop(self.reap());
        if released {
            self.join_outstanding_readers();
        }
        // If the group could not be released, some descendant may still hold a
        // pipe open and joining would block the caller for that descendant's
        // whole lifetime. Two leaked threads is the lesser failure of the two,
        // and getting here needs `kill(2)` to fail on a group this process
        // created with a signal it is permitted to send, which is not a state
        // the kernel produces in practice.
    }
}
