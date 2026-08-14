//! Tests for real command execution, timeout handling, output limits, and error mapping.
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
fn run_accepts_output_exactly_at_the_limit() {
    // The ceiling is inclusive: output of exactly the limit is captured whole.
    let runner = RealCommandRunner;
    let spec = CommandSpec::new("printf")
        .args(["hello"])
        .max_output_bytes(5);
    let output = runner.run(&spec).expect("output at the limit is captured");
    assert_eq!(output.stdout, "hello");
}
