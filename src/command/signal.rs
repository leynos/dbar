//! Serializing the process-group kill against the child's reap.
//!
//! Two places terminate the child's process group: a bounded reader thread that
//! has watched its stream overrun its ceiling, and the session itself on the
//! timeout, failure, and drop paths. Neither can be dropped — the reader's kill
//! is what unblocks the session's wait when the child would otherwise sleep out
//! its timeout — but nothing used to coordinate them. Both could fire, and a
//! reader could fire at any moment, including after the wait had already reaped
//! the child and given the pid back to the kernel.
//!
//! [`SignalClaim`] is the token both sides must win before signalling. It is a
//! `Mutex` rather than an atomic flag because recording the reap must not
//! overtake a kill that is already in flight: winning a `compare_exchange` and
//! then calling `kill(2)` leaves a window in which the reap is recorded between
//! the two, so the signal still lands afterwards. Taking the lock for the whole
//! of the decision and the syscall closes that. Holding it across `kill(2)` is
//! cheap, because sending a signal does not block.

use std::io;
use std::sync::{Mutex, PoisonError};

use super::CommandError;

/// Terminate the child and every descendant sharing its process group.
///
/// Signalling only the direct child would leave a backgrounded grandchild
/// holding the inherited pipe write end open, so the reader threads would never
/// observe EOF and the timeout would block instead of returning.
///
/// The pid is taken raw rather than as a `&Child` so that a reader thread,
/// which does not own the child handle, can terminate the group too.
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

/// Which of the two signalling paths is asking.
///
/// The distinction is not cosmetic: once the child is reaped the session is
/// still allowed one deliberate kill, to evict descendants that inherited the
/// pipes, whereas a reader has no business signalling a pid the session has
/// already handed back.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Signaller {
    /// A bounded reader thread that saw its stream overrun its ceiling.
    Reader,
    /// The session, on its timeout, failure, or drop path.
    Session,
}

/// How much of the claim is left to spend.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ClaimState {
    /// Nothing has signalled yet; whoever claims first may.
    Unsignalled,
    /// The group has been killed. Nobody may kill it again, because a second
    /// kill after the reap would be aimed at whatever now owns the pid.
    Signalled,
    /// The child has been reaped without the group having been killed. Only the
    /// session may still signal, and only once: that kill is what evicts a
    /// descendant still holding the pipes on the ordinary success path.
    Reaped,
    /// Reaped and killed. No further signal is permitted from either side.
    Sealed,
}

/// The shared right to kill the child's process group, held by the session and
/// by both reader threads.
pub(super) struct SignalClaim {
    /// Cached at spawn because `Child::id` is meaningless once the child is
    /// reaped, and the group kill needs the pid afterwards.
    pid: u32,
    /// The claim itself. See the module docs for why this is a lock.
    state: Mutex<ClaimState>,
    /// Kills issued by a reader thread after the reap was recorded. Zero by
    /// construction here; a test asserts it, so that removing the claim is a
    /// visible regression rather than a silent one.
    #[cfg(test)]
    post_reap_reader_signals: std::sync::atomic::AtomicUsize,
}

impl SignalClaim {
    /// Open a claim on a freshly spawned child's group.
    pub(super) const fn new(pid: u32) -> Self {
        Self {
            pid,
            state: Mutex::new(ClaimState::Unsignalled),
            #[cfg(test)]
            post_reap_reader_signals: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Run `act` against the claim state.
    ///
    /// Poisoning is recovered from rather than propagated: the state is a plain
    /// enum that every path leaves consistent, so a panic elsewhere in the
    /// process says nothing about it, and refusing to signal here would leak a
    /// process group.
    fn with_state<T>(&self, act: impl FnOnce(&mut ClaimState) -> T) -> T {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        act(&mut state)
    }

    /// Whether `who` may spend the claim from `state`, and what that leaves.
    const fn next_state(state: ClaimState, who: Signaller) -> Option<ClaimState> {
        match (state, who) {
            (ClaimState::Unsignalled, _) => Some(ClaimState::Signalled),
            (ClaimState::Reaped, Signaller::Session) => Some(ClaimState::Sealed),
            _ => None,
        }
    }

    /// Count a kill that a reader issued after the reap.
    #[cfg(test)]
    fn record(&self, who: Signaller, previous: ClaimState) {
        if matches!(who, Signaller::Reader)
            && matches!(previous, ClaimState::Reaped | ClaimState::Sealed)
        {
            self.post_reap_reader_signals
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// Kills a reader issued after the reap was recorded.
    #[cfg(test)]
    pub(super) fn post_reap_reader_signals(&self) -> usize {
        self.post_reap_reader_signals
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Kill the process group if, and only if, `who` wins the claim.
    ///
    /// A caller that does not win has nothing to do: either the group is
    /// already dead or the pid is no longer safe to aim at, and both are the
    /// state the caller wanted. A failed kill hands the claim back, so the
    /// session's own cleanup can still try.
    pub(super) fn signal(&self, who: Signaller) -> Result<(), CommandError> {
        self.with_state(|state| {
            let previous = *state;
            let Some(next) = Self::next_state(previous, who) else {
                return Ok(());
            };
            *state = next;
            #[cfg(test)]
            self.record(who, previous);
            signal_process_group(self.pid).inspect_err(|_| *state = previous)
        })
    }

    /// Record that the direct child has been reaped.
    ///
    /// After this the claim is worth at most one more kill, and only to the
    /// session. If the group was already killed there is nothing left to evict,
    /// so the claim is spent outright.
    pub(super) fn mark_reaped(&self) {
        self.with_state(|state| {
            *state = match *state {
                ClaimState::Unsignalled => ClaimState::Reaped,
                ClaimState::Signalled | ClaimState::Reaped | ClaimState::Sealed => {
                    ClaimState::Sealed
                }
            };
        });
    }
}
