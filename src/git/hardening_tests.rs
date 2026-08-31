//! End-to-end proof that probing a repository does not run its own commands.
//!
//! A repository carries its own `.git/config`, and the probed directory is
//! wherever the user's shell happens to be, so configuration keys that name a
//! command for git to run are attacker-controlled whenever the checkout is
//! untrusted.
//!
//! These tests run the real `git` binary against a real temporary repository
//! rather than comparing `CommandSpec` values. The question at issue is not
//! whether the hardening reaches the argument list — it is whether git obeys
//! it — and a spec assertion would keep passing if the options went stale
//! against a future git.
//!
//! Each test carries its own negative control: having asserted that the probe
//! left no marker, it re-runs the same repository through a deliberately
//! unhardened command and asserts the marker *does* appear. Without that, a
//! fixture that had quietly stopped triggering the vector would still pass.
//!
//! The fixture is hermetic without touching process-wide state: the isolating
//! `GIT_CONFIG_*` variables are set on each child process rather than on this
//! one, so a developer's own configuration cannot reach the fixture and the
//! tests need no serialization.

use std::io;
use std::process::Command;

use camino::{Utf8Path, Utf8PathBuf};
use cap_std::ambient_authority;
use cap_std::fs_utf8::{Dir, Permissions, PermissionsExt as _};
use rstest::rstest;
use tempfile::TempDir;

use super::{GitStatusOutcome, git_status};
use crate::command::{CommandRunner as _, CommandSpec, RealCommandRunner};

/// The file a successful injection creates. It sits beside the repository
/// rather than inside it, so it cannot be mistaken for an untracked entry.
const MARKER: &str = "executed";

/// The tracked file whose stat information the probes refresh.
const TRACKED: &str = "tracked.txt";

/// The directory an armed `core.hooksPath` points at.
const HOOKS: &str = "hooks";

/// The hook `git status` runs when refreshing the index rewrites it.
const HOOK: &str = "post-index-change";

/// A temporary repository armed with one repository-controlled command.
struct HostileRepo {
    /// Retained so the directory outlives the test and is removed after it.
    _temp: TempDir,
    /// The temporary root; holds the repository and the marker side by side.
    root: Utf8PathBuf,
    /// The repository the probes are pointed at.
    repo: Utf8PathBuf,
}

impl HostileRepo {
    /// Initialize a repository holding one staged file and no commits.
    ///
    /// No commit is needed: `git status` refreshes and rewrites the index for a
    /// staged file, which is enough to reach both vectors under test.
    /// Arranging state is not testing it, so setup failures are propagated
    /// rather than unwrapped here. The test bodies stay `()` and unwrap the
    /// result themselves, which keeps a broken fixture distinguishable from a
    /// failed assertion.
    fn new() -> io::Result<Self> {
        let temp = TempDir::new()?;
        let root = Utf8PathBuf::from_path_buf(temp.path().to_path_buf())
            .map_err(|_| io::Error::other("temporary directory path is not UTF-8"))?;
        run_git(&root, &["init", "--quiet", "repo"])?;
        let repo = root.join("repo");
        let fixture = Self {
            _temp: temp,
            root,
            repo,
        };
        fixture.restat()?;
        run_git(&fixture.repo, &["add", TRACKED])?;
        Ok(fixture)
    }

    /// The shell command an armed vector runs if git ever invokes it.
    ///
    /// `sh -c` swallows the arguments git appends, so the injection leaves
    /// nothing in the worktree except the marker.
    fn injected_command(&self) -> String {
        let marker = self.root.join(MARKER);
        format!("sh -c 'touch \"{marker}\"'")
    }

    /// Arm `core.fsmonitor`, which names a filesystem-monitor command that
    /// `git status` runs during its refresh.
    fn arm_fsmonitor(&self) -> io::Result<()> {
        let command = self.injected_command();
        run_git(&self.repo, &["config", "core.fsmonitor", &command])
    }

    /// Arm `core.hooksPath`, redirecting hook lookup at a directory holding an
    /// executable [`HOOK`].
    fn arm_hooks_path(&self) -> io::Result<()> {
        open_dir(&self.root)?.create_dir(HOOKS)?;
        let hooks_dir = self.root.join(HOOKS);
        let script = format!("#!/bin/sh\n{}\n", self.injected_command());
        write_file(&hooks_dir, HOOK, &script)?;
        open_dir(&hooks_dir)?.set_permissions(HOOK, Permissions::from_mode(0o755))?;
        run_git(
            &self.repo,
            &["config", "core.hooksPath", hooks_dir.as_str()],
        )
    }

    /// Configure an attribute-selected clean filter for the tracked file.
    fn arm_filter(&self) -> io::Result<()> {
        write_file(&self.repo, ".gitattributes", "tracked.txt filter=hostile\n")?;
        let command = self.injected_command();
        run_git(
            &self.repo,
            &["config", "filter.hostile.clean", &format!("{command}; cat")],
        )
    }

    /// Rewrite the tracked file so its stat information no longer matches the
    /// index, which is what makes the next `git status` refresh and rewrite it.
    ///
    /// The contents are unchanged, so the file stays clean and the flags the
    /// probes report are unaffected.
    fn restat(&self) -> io::Result<()> {
        write_file(&self.repo, TRACKED, "contents\n")
    }

    /// Whether the injected command ran.
    fn fired(&self) -> bool {
        open_dir(&self.root).is_ok_and(|dir| dir.metadata(MARKER).is_ok())
    }
}

/// Open a directory through `cap_std`, since `std::fs` is barred from `src/`.
fn open_dir(path: &Utf8Path) -> io::Result<Dir> {
    Dir::open_ambient_dir(path, ambient_authority())
}

/// Write a fixture file through `cap_std`.
fn write_file(dir: &Utf8Path, name: &str, contents: &str) -> io::Result<()> {
    open_dir(dir)?.write(name, contents)
}

/// Run one `git` invocation while building the fixture.
fn run_git(cwd: &Utf8Path, args: &[&str]) -> io::Result<()> {
    let status = Command::new("git")
        .args(args)
        .current_dir(cwd.as_std_path())
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("git {args:?} failed in {cwd}")))
    }
}

/// Probe the fixture, then prove the fixture could have fired.
///
/// The control spec is built by hand rather than through `git_command`,
/// because the point is to issue the command the probe made *before* the
/// hardening existed.
///
/// A macro rather than a function: it has to both assert and unwrap a setup
/// step, and no function can do both here. Returning `Result` to carry the
/// setup error would trip `panic_in_result_fn` on the assertions, while
/// unwrapping inside a plain helper trips `no_expect_outside_tests`, since a
/// helper is not recognised as a test. Expanding inline puts everything in the
/// test body, where both are allowed — and keeps failure line numbers pointing
/// at the calling test rather than at a shared helper.
macro_rules! assert_probe_is_inert {
    ($fixture:expr, $vector:expr) => {{
        let fixture: &HostileRepo = $fixture;
        let vector: &str = $vector;
        assert!(!fixture.fired(), "the fixture must start clean");

        let outcome = git_status(&RealCommandRunner, &fixture.repo);

        assert!(
            !fixture.fired(),
            "probing ran the command named by `{vector}`"
        );
        // The hardening must not have cost the answer: the staged file still
        // registers, so nothing was disabled that changes what git considers
        // changed.
        let GitStatusOutcome::Available(report) = outcome else {
            panic!("expected the temporary repository to probe as available");
        };
        assert!(
            report.status.staged,
            "the staged file should still register"
        );

        fixture.restat().expect("restat the tracked file");
        let unhardened = CommandSpec::new("git")
            .args(["status", "--porcelain"])
            .cwd(fixture.repo.clone());
        // Only the side effect matters here, not what the command reported.
        drop(RealCommandRunner.run(&unhardened));
        assert!(
            fixture.fired(),
            "the fixture no longer triggers `{vector}`, so the assertion above is vacuous"
        );
    }};
}

#[rstest]
fn probing_does_not_run_a_repositorys_fsmonitor_command() {
    let fixture = HostileRepo::new().expect("build the hostile fixture");
    fixture.arm_fsmonitor().expect("arm core.fsmonitor");
    assert_probe_is_inert!(&fixture, "core.fsmonitor");
}

#[rstest]
fn probing_does_not_run_a_repositorys_index_hook() {
    let fixture = HostileRepo::new().expect("build the hostile fixture");
    fixture.arm_hooks_path().expect("arm core.hooksPath");
    assert_probe_is_inert!(&fixture, "core.hooksPath");
}

#[rstest]
fn probing_skips_a_worktree_status_with_an_executable_filter() {
    let fixture = HostileRepo::new().expect("build the hostile fixture");
    fixture.arm_filter().expect("arm the attribute filter");

    let outcome = git_status(&RealCommandRunner, &fixture.repo);

    assert!(
        !fixture.fired(),
        "probing ran the filter selected by repository attributes"
    );
    let GitStatusOutcome::Available(report) = outcome else {
        panic!("the safe probes should still produce a git report");
    };
    assert!(!report.status.dirty);
    assert!(!report.status.staged);
    assert!(
        report.degradations.iter().any(|failure| matches!(
            failure,
            super::GitProbeFailure::FilterConfigured {
                probe: super::GitProbe::WorktreeStatus,
            }
        )),
        "skipping the unsafe worktree probe must remain diagnosable"
    );

    let unhardened = CommandSpec::new("git")
        .args(["status", "--porcelain"])
        .cwd(fixture.repo.clone());
    drop(RealCommandRunner.run(&unhardened));
    assert!(
        fixture.fired(),
        "the fixture must still prove that an unhardened status invokes the filter"
    );
}
