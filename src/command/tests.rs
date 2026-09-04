//! Tests for real command execution, timeout handling, output limits, and error mapping.
//!
//! Every case spawns a POSIX utility (`printf`, `false`, `sh`, `sleep`) and the
//! process-group assertions describe Unix semantics, which needs no `cfg` gate:
//! the crate refuses to build off Unix, as `src/command/mod.rs` explains.
use super::*;
use rstest::rstest;
use std::time::Instant;

use super::child::{ReadOutcome, readers_joined};

/// Wait for every process in `pgid`'s group to disappear, up to `deadline`.
///
/// A killed grandchild is reparented and reaped asynchronously, so a single
/// probe would race that reap and flake. Polling to a deadline tolerates the
/// delay while still failing a cleanup that never signalled the group at all.
/// A pid that will not convert cannot be probed, which is reported as failure
/// rather than as a group that vanished.
fn group_exits_within(pgid: u32, deadline: Duration) -> bool {
    let Ok(raw) = i32::try_from(pgid) else {
        return false;
    };
    let Some(group) = rustix::process::Pid::from_raw(raw) else {
        return false;
    };
    let started = Instant::now();
    loop {
        // `ESRCH` is the only answer that means the group is empty; `EPERM`
        // would mean the pid now belongs to somebody else, so keep waiting.
        if matches!(
            rustix::process::test_kill_process_group(group),
            Err(rustix::io::Errno::SRCH)
        ) {
            return true;
        }
        if started.elapsed() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Whether the direct child has been reaped, leaving no zombie behind.
///
/// `ECHILD` means this process has no such child left to wait for, which is
/// exactly what a reap leaves behind; an unreaped child would still be
/// waitable as a zombie.
fn child_was_reaped(pid: u32) -> bool {
    let Ok(raw) = i32::try_from(pid) else {
        return false;
    };
    let Some(child) = rustix::process::Pid::from_raw(raw) else {
        return false;
    };
    matches!(
        rustix::process::waitpid(Some(child), rustix::process::WaitOptions::NOHANG),
        Err(rustix::io::Errno::CHILD)
    )
}

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

#[rstest]
fn run_rejects_stdout_beyond_the_limit() {
    // The child floods stdout then sleeps far past the assertion window. The
    // reader must abandon the capture and kill the process group rather than
    // waiting the child out, so the call returns long before the timeout.
    let runner = RealCommandRunner;
    let spec = CommandSpec::new("sh")
        .args(["-c", "head -c 200000 /dev/zero | tr '\\0' 'x'; sleep 30"])
        .max_output_bytes(1024)
        .timeout(Duration::from_secs(20));

    let started = Instant::now();
    let err = runner
        .run(&spec)
        .expect_err("oversized stdout must be rejected");
    let elapsed = started.elapsed();

    assert!(
        matches!(
            err,
            CommandError::OutputTooLarge {
                stream: "stdout",
                ..
            }
        ),
        "expected an stdout limit breach, got {err:?}"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "the child must be killed rather than waited out, took {elapsed:?}"
    );
}

#[rstest]
fn run_rejects_stderr_beyond_the_limit() {
    // As above, but the flood is redirected to stderr, which is bounded by the
    // same ceiling and reported against its own stream name.
    let runner = RealCommandRunner;
    let spec = CommandSpec::new("sh")
        .args([
            "-c",
            "head -c 200000 /dev/zero | tr '\\0' 'x' >&2; sleep 30",
        ])
        .max_output_bytes(1024)
        .timeout(Duration::from_secs(20));

    let started = Instant::now();
    let err = runner
        .run(&spec)
        .expect_err("oversized stderr must be rejected");
    let elapsed = started.elapsed();

    assert!(
        matches!(
            err,
            CommandError::OutputTooLarge {
                stream: "stderr",
                ..
            }
        ),
        "expected an stderr limit breach, got {err:?}"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "the child must be killed rather than waited out, took {elapsed:?}"
    );
}

#[rstest]
fn dropping_a_session_terminates_the_child_and_joins_its_readers() {
    // Every error return between the spawn and the joins now does exactly what
    // this test does: nothing, and lets the guard clean up. The child stalls far
    // past the assertion window and backgrounds a grandchild holding the
    // inherited pipes, so a guard that skipped the group kill would block in the
    // reader join for the grandchild's whole lifetime, and a guard that returned
    // before joining would be the leak this replaces.
    let mut command = Command::new("sh");
    command.args(["-c", "sleep 30 & sleep 30"]);
    capture_output(&mut command);
    use_own_process_group(&mut command);
    let child = command.spawn().expect("sh spawns");
    // The child leads its own group, so its pid doubles as the group id.
    let pid = child.id();
    let mut session = ChildSession::new(child);
    session.start_readers(1024).expect("both readers start");

    let joined_before = readers_joined();
    let started = Instant::now();
    drop(session);
    let elapsed = started.elapsed();

    assert_eq!(
        readers_joined().saturating_sub(joined_before),
        2,
        "dropping the session must join both readers, not abandon them"
    );
    assert!(
        child_was_reaped(pid),
        "dropping the session must reap the direct child"
    );
    assert!(
        group_exits_within(pid, Duration::from_secs(5)),
        "dropping the session must terminate the whole process group"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "dropping the session must not wait the child out, took {elapsed:?}"
    );
}

#[rstest]
fn run_captures_output_under_a_maximal_limit() {
    // A ceiling of `usize::MAX` leaves no room for the extra sentinel byte that
    // detects an overrun. Wrapping the addition would read zero bytes and report
    // empty output as a success; saturating reads the stream out in full.
    let runner = RealCommandRunner;
    let spec = CommandSpec::new("printf")
        .args(["hello"])
        .max_output_bytes(usize::MAX);
    let output = runner
        .run(&spec)
        .expect("a maximal limit captures output whole");
    assert_eq!(output.stdout, "hello");
}

#[rstest]
fn run_accepts_output_exactly_at_the_limit() {
    // The ceiling is inclusive: output of exactly the limit is captured whole.
    let runner = RealCommandRunner;
    let spec = CommandSpec::new("printf")
        .args(["hello"])
        .max_output_bytes(5);
    let output = runner.run(&spec).expect("output at the limit is captured");
    assert_eq!(output.stdout, "hello");
}

#[rstest]
fn a_reader_does_not_signal_the_group_after_the_child_is_reaped() {
    // The overrun kill and the session's own kill are separate signalling
    // paths, and a reader has no way of its own to know the child has already
    // been reaped. Ordering the two by hand makes the hazard deterministic
    // rather than waiting on a race: 2000 bytes fits inside the 64 KiB pipe
    // buffer, so the child writes the lot and exits without blocking, the data
    // outlives it in the pipe, and reaping before the readers start guarantees
    // the overrun is detected strictly after the reap. Without the shared
    // claim the reader kills a pid the kernel has already taken back.
    let mut command = Command::new("sh");
    command.args(["-c", "head -c 2000 /dev/zero | tr '\\0' 'x'"]);
    capture_output(&mut command);
    use_own_process_group(&mut command);
    let child = command.spawn().expect("sh spawns");
    let mut session = ChildSession::new(child);

    let status = session
        .wait_for(Duration::from_secs(20))
        .expect("the wait succeeds");
    assert!(status.is_some(), "the child must exit within the wait");

    session.start_readers(1024).expect("both readers start");
    let (stdout, _stderr) = session.join_readers().expect("both readers join");

    assert!(
        matches!(stdout, ReadOutcome::TooLarge),
        "the buffered output must still be seen to overrun its ceiling"
    );
    assert_eq!(
        session.post_reap_reader_signals(),
        0,
        "a reader must not kill a process group whose pid has been given back"
    );
}
