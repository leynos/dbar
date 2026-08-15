//! The individual `git` probes behind [`super::git_status`].
//!
//! Each probe runs one `git` invocation, parses its output, and reports
//! either the parsed value or the typed failure that forced a fallback. The
//! fallbacks themselves are documented in the parent module.

use camino::Utf8Path;

use super::{GitProbe, GitProbeFailure};
use crate::command::{CommandError, CommandRunner, CommandSpec};
use crate::types::{AheadCount, BehindCount, BranchName, ProjectName};

/// A probed value paired with the failure, if any, that forced its fallback.
pub(super) struct Probed<T> {
    /// The value to use, already defaulted if the probe degraded.
    pub(super) value: T,
    /// The failure the fallback absorbed.
    pub(super) failure: Option<GitProbeFailure>,
}

impl<T> Probed<T> {
    /// A probe that succeeded outright.
    const fn ok(value: T) -> Self {
        Self {
            value,
            failure: None,
        }
    }

    /// A probe that fell back to `value` because of `failure`.
    const fn degraded(value: T, failure: GitProbeFailure) -> Self {
        Self {
            value,
            failure: Some(failure),
        }
    }
}

/// Build a `git` command spec rooted at the given project directory.
pub(super) fn git_command(
    project_dir: &Utf8Path,
    args: impl IntoIterator<Item = impl Into<String>>,
) -> CommandSpec {
    CommandSpec::new("git")
        .args(args)
        .cwd(project_dir.to_path_buf())
}

/// Run one probe, tagging any command failure with the probe that raised it.
fn run_probe(
    runner: &dyn CommandRunner,
    project_dir: &Utf8Path,
    probe: GitProbe,
    args: impl IntoIterator<Item = impl Into<String>>,
) -> Result<String, GitProbeFailure> {
    let spec = git_command(project_dir, args);
    runner
        .run(&spec)
        .map(|output| output.stdout)
        .map_err(|source| GitProbeFailure::CommandFailed { probe, source })
}

/// Ask git whether `project_dir` is inside a work tree.
pub(super) fn probe_repository(
    runner: &dyn CommandRunner,
    project_dir: &Utf8Path,
) -> Result<bool, GitProbeFailure> {
    let probe = GitProbe::Repository;
    let stdout = run_probe(
        runner,
        project_dir,
        probe,
        ["rev-parse", "--is-inside-work-tree"],
    )?;
    match stdout.trim() {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(GitProbeFailure::MalformedOutput {
            probe,
            output: other.to_owned(),
        }),
    }
}

/// Read the current branch, reporting `None` when there is not one.
///
/// No name is invented here. An empty answer means a detached `HEAD`, and a
/// failed probe means git could not say; both leave the branch absent so that
/// the renderer's `detached` label stays a rendering decision and cannot be
/// mistaken for a real branch of that name further downstream.
pub(super) fn probe_branch(
    runner: &dyn CommandRunner,
    project_dir: &Utf8Path,
) -> Probed<Option<BranchName>> {
    let probe = GitProbe::Branch;
    match run_probe(runner, project_dir, probe, ["branch", "--show-current"]) {
        // An empty answer is git's documented way of saying "detached HEAD",
        // so it is a legitimate result rather than a degradation.
        Ok(stdout) if stdout.trim().is_empty() => Probed::ok(None),
        Ok(stdout) => Probed::ok(Some(BranchName::new(stdout.trim().to_owned()))),
        Err(failure) => Probed::degraded(None, failure),
    }
}

/// Read the project name from the `origin` remote URL.
///
/// A non-zero exit is git *answering*: it is how `git remote get-url` reports
/// that there is no `origin` remote, and how git reports that the directory is
/// not a repository at all. Both are ordinary cases for which the path-derived
/// name is the intended answer, so neither is recorded as a degradation. Every
/// other failure — a missing `git` binary, a timeout, an oversized stream, or
/// a URL whose last segment is empty — is a genuine probe failure and is
/// reported.
pub(super) fn probe_origin_name(
    runner: &dyn CommandRunner,
    project_dir: &Utf8Path,
) -> Probed<Option<ProjectName>> {
    let probe = GitProbe::OriginUrl;
    let stdout = match run_probe(runner, project_dir, probe, ["remote", "get-url", "origin"]) {
        Ok(value) => value,
        Err(GitProbeFailure::CommandFailed {
            source: CommandError::NonZero { .. },
            ..
        }) => return Probed::ok(None),
        Err(failure) => return Probed::degraded(None, failure),
    };

    parse_origin_name(&stdout).map_or_else(
        || {
            Probed::degraded(
                None,
                GitProbeFailure::MalformedOutput {
                    probe,
                    output: stdout.trim().to_owned(),
                },
            )
        },
        |name| Probed::ok(Some(name)),
    )
}

/// Take the repository name off the end of a remote URL.
fn parse_origin_name(origin: &str) -> Option<ProjectName> {
    let trimmed = origin.trim();
    let name = trimmed.rsplit(&['/', ':'][..]).next()?;
    let cleaned = name.trim_end_matches(".git");
    if cleaned.is_empty() {
        None
    } else {
        Some(ProjectName::new(cleaned.to_owned()))
    }
}

/// Read the dirty and staged flags from `git status --porcelain`.
pub(super) fn probe_worktree_status(
    runner: &dyn CommandRunner,
    project_dir: &Utf8Path,
) -> Probed<(bool, bool)> {
    let probe = GitProbe::WorktreeStatus;
    let stdout = match run_probe(runner, project_dir, probe, ["status", "--porcelain"]) {
        Ok(value) => value,
        Err(failure) => return Probed::degraded((false, false), failure),
    };

    let summary = parse_porcelain(&stdout);
    let value = (summary.dirty, summary.staged);
    summary.malformed_line.map_or_else(
        || Probed::ok(value),
        |output| Probed::degraded(value, GitProbeFailure::MalformedOutput { probe, output }),
    )
}

/// The dirty and staged flags plus the first unparseable porcelain line.
struct PorcelainSummary {
    /// Whether the worktree has unstaged changes.
    dirty: bool,
    /// Whether the index has staged changes.
    staged: bool,
    /// The first line too short to carry both status characters.
    malformed_line: Option<String>,
}

/// Summarize `git status --porcelain` output.
///
/// Every line is inspected — the scan does not stop once both flags latch —
/// so a malformed line anywhere in the output is still reported. The cost is
/// bounded by `CommandSpec`'s output ceiling, not by this loop.
fn parse_porcelain(stdout: &str) -> PorcelainSummary {
    let mut summary = PorcelainSummary {
        dirty: false,
        staged: false,
        malformed_line: None,
    };
    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        let Some((index, worktree)) = status_chars(line) else {
            summary
                .malformed_line
                .get_or_insert_with(|| line.to_owned());
            continue;
        };
        summary.dirty |= matches!(worktree, 'M' | 'A' | 'D' | 'R' | 'C' | 'U' | '?');
        summary.staged |= matches!(index, 'M' | 'A' | 'D' | 'R' | 'C');
    }
    summary
}

/// The index and worktree status characters of one porcelain line.
fn status_chars(line: &str) -> Option<(char, char)> {
    let mut chars = line.chars();
    let index = chars.next()?;
    let worktree = chars.next()?;
    Some((index, worktree))
}

/// Read the ahead and behind counts relative to the upstream branch.
pub(super) fn probe_upstream_counts(
    runner: &dyn CommandRunner,
    project_dir: &Utf8Path,
) -> Probed<(AheadCount, BehindCount)> {
    let probe = GitProbe::UpstreamCounts;
    let fallback = (AheadCount::new(0), BehindCount::new(0));
    let stdout = match run_probe(
        runner,
        project_dir,
        probe,
        ["rev-list", "--left-right", "--count", "@{upstream}...HEAD"],
    ) {
        Ok(value) => value,
        Err(failure) => return Probed::degraded(fallback, failure),
    };

    parse_upstream_counts(&stdout).map_or_else(
        || {
            Probed::degraded(
                fallback,
                GitProbeFailure::MalformedOutput {
                    probe,
                    output: stdout.trim().to_owned(),
                },
            )
        },
        Probed::ok,
    )
}

/// Parse the `behind<TAB>ahead` pair emitted by `rev-list --left-right`.
///
/// `None` means "unparseable", which the caller turns into a
/// [`GitProbeFailure::MalformedOutput`] carrying the raw output; nothing is
/// discarded here, because the `ParseIntError` says no more than the raw
/// output already does.
fn parse_upstream_counts(stdout: &str) -> Option<(AheadCount, BehindCount)> {
    let mut parts = stdout.split_whitespace();
    let behind = parts.next()?.parse::<u32>().ok()?;
    let ahead = parts.next()?.parse::<u32>().ok()?;
    Some((AheadCount::new(ahead), BehindCount::new(behind)))
}
