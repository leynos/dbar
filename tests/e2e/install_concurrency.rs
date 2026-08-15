//! Multi-process race coverage for `dbar install`.
//!
//! `concurrent_installs_leave_one_well_formed_block` in `src/install/tests.rs`
//! races installs across threads, which only exercises the lock within a single
//! process. This module races real `dbar install` processes, so the `flock`
//! has to hold across process boundaries — the property that actually matters
//! for two shells running `dbar install` at once.

use std::io;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use camino::Utf8Path;
use tempfile::TempDir;
use wait_timeout::ChildExt as _;

use super::install_support::{
    MARKER_END, MARKER_START, PROLOGUE, SEEDED_TOKEN, backup_path, config_path, count_occurrences,
    epilogue, read_config, seeded_config, write_config,
};

/// The flags that distinguish one racer's install request from another's.
#[derive(Clone, Copy)]
struct Request {
    /// Value passed to `--position`.
    position: &'static str,
    /// Whether `--full` is passed.
    full: bool,
}

/// Every distinct request the racers issue; each yields a distinct snippet.
const REQUESTS: [Request; 4] = [
    Request {
        position: "left",
        full: false,
    },
    Request {
        position: "left",
        full: true,
    },
    Request {
        position: "right",
        full: false,
    },
    Request {
        position: "right",
        full: true,
    },
];

/// How many processes race. They cycle through [`REQUESTS`], so several
/// conflicting requests are always in flight at once.
const RACER_COUNT: usize = 12;

/// Bound on how long any one racer may run before the test kills it.
///
/// The install lock's own retry budget is about a second, so a racer that
/// outlives this has deadlocked rather than merely queued; failing loudly here
/// keeps a lock regression from wedging CI.
const RACER_TIMEOUT: Duration = Duration::from_secs(30);

/// Racing installs must serialize into one coherent snippet, losing nothing.
///
/// The seeded config carries user configuration on both sides of a stale
/// managed block, so every racer must read it, back it up, and rewrite the
/// block. Under the lock each racer's read-backup-write cycle is one
/// transaction, so the racers apply in some serial order; the assertions below
/// pin the observable consequences of that ordering.
#[test]
fn concurrent_install_processes_leave_one_coherent_snippet() {
    let candidates = solo_outcomes().expect("derive the per-request candidate configs");

    let temp_dir = TempDir::new().expect("temp dir");
    let config = config_path(&temp_dir).expect("config path");
    write_config(&config, &seeded_config()).expect("seed the config");

    run_racers(&config).expect("race the install processes");

    let final_config = read_config(&config).expect("read the config the racers left");
    assert_single_block(&final_config, "the final config");
    assert!(
        candidates.contains(&final_config),
        "the surviving config must match one racer's request exactly, not a mixture of two; its block was: {:?}",
        managed_block(&final_config)
    );

    let backup = backup_path(&config);
    assert!(
        backup.as_std_path().exists(),
        "every racer overwrites the seed, so a backup must have been written"
    );
    assert_backup_is_a_racers_work(
        &read_config(&backup).expect("read the surviving backup"),
        &candidates,
        &final_config,
    );
}

/// Assert the surviving backup records a racer's output, not the seed.
///
/// This is where the cross-process lock is actually observable. Only the first
/// racer in the serial order can back up the seed; every later racer backs up
/// its predecessor's output, and at least two racers update here because their
/// requests differ. So the last backup written — the one that survives at the
/// single fixed backup path — is always some racer's output, and is always the
/// state the final config replaced. Without the lock, two racers can read the
/// seed at once and both back it up, leaving the seed at the backup path while
/// the config has moved on: one racer's update was overwritten unrecorded.
#[track_caller]
fn assert_backup_is_a_racers_work(backup: &str, candidates: &[String], final_config: &str) {
    assert_single_block(backup, "the backup");
    assert!(
        !backup.contains(SEEDED_TOKEN),
        "the surviving backup still holds the seed, so two racers backed up the same state: their read-modify-write cycles overlapped"
    );
    assert!(
        candidates.contains(&backup.to_owned()),
        "the surviving backup must be some racer's output; its block was: {:?}",
        managed_block(backup)
    );
    assert_ne!(
        managed_block(backup),
        managed_block(final_config),
        "the last racer to update backs up its predecessor's output, which cannot equal its own"
    );
}

/// The managed block alone, so diagnostics never dump the whole large config.
fn managed_block(contents: &str) -> &str {
    let Some((_, rest)) = contents.split_once(MARKER_START) else {
        return "<no managed block>";
    };
    rest.split_once(MARKER_END)
        .map_or("<unterminated managed block>", |(block, _)| block)
}

/// Assert `contents` holds exactly one managed block inside intact user config.
#[track_caller]
fn assert_single_block(contents: &str, label: &str) {
    assert_eq!(
        count_occurrences(contents, MARKER_START),
        1,
        "{label} must hold exactly one opening marker"
    );
    assert_eq!(
        count_occurrences(contents, MARKER_END),
        1,
        "{label} must hold exactly one closing marker"
    );
    assert!(
        contents.starts_with(PROLOGUE),
        "{label} must preserve the user's prologue verbatim"
    );
    assert!(
        contents.ends_with(&epilogue()),
        "{label} must preserve the user's epilogue verbatim"
    );
}

/// The exact file each request produces installing against the seed alone.
///
/// Deriving the candidates by running the binary keeps the snippet text out of
/// this test, and gives the race a precise target: any final file outside this
/// set is a mixture of two racers rather than one racer's work.
fn solo_outcomes() -> io::Result<Vec<String>> {
    REQUESTS.iter().copied().map(solo_outcome).collect()
}

fn solo_outcome(request: Request) -> io::Result<String> {
    let temp_dir = TempDir::new()?;
    let config = config_path(&temp_dir)?;
    write_config(&config, &seeded_config())?;
    let output = Command::new(binary())
        .args(args_for(&config, request))
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "solo install failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    read_config(&config)
}

/// Start every racer, release them together, and wait under a bounded timeout.
fn run_racers(config: &Utf8Path) -> io::Result<()> {
    let (spawned, spawn_failures): (Vec<_>, Vec<_>) = REQUESTS
        .iter()
        .copied()
        .cycle()
        .take(RACER_COUNT)
        .map(|request| spawn_racer(config, request))
        .partition(Result::is_ok);
    let mut children: Vec<Child> = spawned.into_iter().filter_map(Result::ok).collect();
    release_racers(&mut children);
    // Wait on every racer that did start, whatever else went wrong, so none
    // outlives the test.
    let waited: Vec<io::Result<()>> = children.into_iter().map(wait_for_racer).collect();
    spawn_failures
        .into_iter()
        .collect::<io::Result<Vec<Child>>>()?;
    waited.into_iter().collect()
}

/// Release every parked racer as tightly as the platform allows.
///
/// Spawn order alone is not a race: forking a debug binary costs milliseconds
/// and an install costs less, so racers started in a loop simply queue up and
/// never contend. Each racer is instead parked in a shell reading its stdin,
/// and this releases them all before any of them can finish.
///
/// Either a byte or an end of file frees the shell's `read`, so closing the
/// pipes alone would be enough to *start* every racer, and is the tidier
/// spelling. It is nonetheless not what this does, because it measurably stops
/// the test working. With `acquire_lock` disabled, the close-only form passed
/// ten runs out of ten — detecting nothing on a build with no locking at all —
/// where writing first and then closing failed the run in 4 of 10, and 5 of 5
/// in a later measurement. Releasing each racer as its byte lands bunches the
/// twelve processes tightly enough that they overlap inside the
/// read-modify-write window; releasing them as the drops are scheduled does
/// not, and they stagger past each other instead.
///
/// So the write is load-bearing and must stay, even though the pass that
/// follows it would suffice on its own. Neither pass is truly simultaneous;
/// the write is simply the tighter of the two. A failed write is harmless and
/// is deliberately ignored: the close behind it still delivers the end of file.
fn release_racers(children: &mut [Child]) {
    use std::io::Write as _;
    for child in children.iter_mut() {
        if let Some(stdin) = child.stdin.as_mut() {
            stdin.write_all(b"\n").ok();
        }
    }
    for child in children.iter_mut() {
        drop(child.stdin.take());
    }
}

/// Park a racer in a POSIX shell that waits on stdin, then `exec`s `dbar`.
///
/// The shell has already been forked and scheduled by the time it blocks on the
/// read, so [`release_racers`] starts all the installs together. `git` is
/// already a hard requirement of these tests, so relying on `sh` here costs
/// nothing extra.
fn spawn_racer(config: &Utf8Path, request: Request) -> io::Result<Child> {
    Command::new("sh")
        .arg("-c")
        .arg("IFS= read -r _ || true; exec \"$@\"")
        .arg("dbar-racer")
        .arg(binary())
        .args(args_for(config, request))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
}

/// Wait for one racer, killing it if it outlives [`RACER_TIMEOUT`].
fn wait_for_racer(mut child: Child) -> io::Result<()> {
    let Some(status) = child.wait_timeout(RACER_TIMEOUT)? else {
        child.kill()?;
        child.wait()?;
        return Err(io::Error::other("install racer exceeded its timeout"));
    };
    if status.success() {
        return Ok(());
    }
    Err(io::Error::other(format!(
        "install racer failed: {}",
        racer_stderr(&mut child)
    )))
}

fn racer_stderr(child: &mut Child) -> String {
    use std::io::Read as _;

    let mut buffer = String::new();
    if let Some(mut stderr) = child.stderr.take() {
        // Diagnostics only: a failed read just yields a vaguer message.
        drop(stderr.read_to_string(&mut buffer));
    }
    buffer
}

fn args_for(config: &Utf8Path, request: Request) -> Vec<String> {
    let mut args = vec![
        "install".to_owned(),
        "--path".to_owned(),
        config.to_string(),
        "--position".to_owned(),
        request.position.to_owned(),
    ];
    if request.full {
        args.push("--full".to_owned());
    }
    args
}

fn binary() -> &'static std::path::Path {
    assert_cmd::cargo::cargo_bin!("dbar")
}
