//! Git probing utilities for dbar.
//!
//! Every probe reports a typed outcome instead of silently substituting a
//! default, so a caller can tell "this directory is not a repository" apart
//! from "the `git` binary is missing" and from "`git` answered with something
//! this module cannot parse".
//!
//! # Fallback policy
//!
//! The rendered status line is unchanged by any of these failures; only the
//! diagnosis differs.
//!
//! | Failure | Rendered result |
//! | --- | --- |
//! | Not a repository ([`GitStatusOutcome::NotARepository`]) | no git segment |
//! | Repository probe failed or was unparseable ([`GitStatusOutcome::Unavailable`]) | no git segment |
//! | Branch probe failed | branch reads `detached` |
//! | Branch probe returned an empty name (a genuinely detached `HEAD`) | branch reads `detached`, no degradation recorded |
//! | Worktree-status probe failed | neither dirty nor staged |
//! | A porcelain line was too short to classify | that line is ignored; the rest still count |
//! | Upstream-count probe failed or was unparseable | ahead and behind both read zero |

use std::fmt;

use camino::Utf8Path;
use thiserror::Error;

use crate::command::{CommandError, CommandRunner};
use crate::types::{AheadCount, BehindCount, BranchName, ProjectName};

mod probes;

use probes::{
    git_command, probe_branch, probe_repository, probe_upstream_counts, probe_worktree_status,
};

#[derive(Debug, Clone)]
/// Snapshot of git status metadata for rendering.
pub struct GitStatus {
    /// The current branch name.
    pub branch: BranchName,
    /// Whether the worktree has unstaged changes.
    pub dirty: bool,
    /// Whether the index contains staged changes.
    pub staged: bool,
    /// Ahead count relative to upstream.
    pub ahead: AheadCount,
    /// Behind count relative to upstream.
    pub behind: BehindCount,
    /// Whether this path looks like a worktree.
    pub is_worktree: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Identifies which `git` probe a failure refers to.
pub enum GitProbe {
    /// `git rev-parse --is-inside-work-tree`.
    Repository,
    /// `git branch --show-current`.
    Branch,
    /// `git status --porcelain`.
    WorktreeStatus,
    /// `git rev-list --left-right --count @{upstream}...HEAD`.
    UpstreamCounts,
}

impl GitProbe {
    /// The `git` sub-command this probe runs, for diagnostics.
    const fn command(self) -> &'static str {
        match self {
            Self::Repository => "rev-parse --is-inside-work-tree",
            Self::Branch => "branch --show-current",
            Self::WorktreeStatus => "status --porcelain",
            Self::UpstreamCounts => "rev-list --left-right --count @{upstream}...HEAD",
        }
    }
}

impl fmt::Display for GitProbe {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "git {}", self.command())
    }
}

#[derive(Debug, Error)]
/// Why a `git` probe did not yield usable data.
pub enum GitProbeFailure {
    /// The probe could not be run, timed out, or exited non-zero.
    #[error("`{probe}` failed: {source}")]
    CommandFailed {
        /// The probe that failed.
        probe: GitProbe,
        /// The underlying command failure.
        source: CommandError,
    },
    /// The probe ran but produced output this module cannot parse.
    #[error("`{probe}` produced unusable output: {output:?}")]
    MalformedOutput {
        /// The probe whose output could not be parsed.
        probe: GitProbe,
        /// The offending output, trimmed to the fragment that failed.
        output: String,
    },
}

#[derive(Debug)]
/// A git snapshot together with the probes that degraded while collecting it.
pub struct GitStatusReport {
    /// The snapshot to render.
    pub status: GitStatus,
    /// Field-level failures that were absorbed by the fallback policy.
    pub degradations: Vec<GitProbeFailure>,
}

#[derive(Debug)]
/// Outcome of [`git_status`].
pub enum GitStatusOutcome {
    /// A snapshot was assembled; individual fields may still have degraded.
    Available(GitStatusReport),
    /// The probe answered, and the path is not inside a git worktree.
    NotARepository,
    /// The repository probe itself failed or could not be parsed.
    Unavailable(GitProbeFailure),
}

impl GitStatusOutcome {
    /// The snapshot to render, if one could be assembled.
    ///
    /// `None` means the status line omits the git segment entirely, which is
    /// the same rendering for "not a repository" and for a failed probe.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use dbar::git::GitStatusOutcome;
    ///
    /// assert!(GitStatusOutcome::NotARepository.status().is_none());
    /// ```
    pub const fn status(&self) -> Option<&GitStatus> {
        match self {
            Self::Available(report) => Some(&report.status),
            Self::NotARepository | Self::Unavailable(_) => None,
        }
    }

    /// Consume the outcome, returning every failure it recorded.
    ///
    /// A successful probe with no degradations yields an empty vector.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use dbar::git::GitStatusOutcome;
    ///
    /// assert!(GitStatusOutcome::NotARepository.into_failures().is_empty());
    /// ```
    pub fn into_failures(self) -> Vec<GitProbeFailure> {
        match self {
            Self::Available(report) => report.degradations,
            Self::NotARepository => Vec::new(),
            Self::Unavailable(failure) => vec![failure],
        }
    }
}

/// Resolve the project name using git metadata and directory heuristics.
///
/// Unlike [`git_status`], this is a chain of *heuristics*, not a chain of
/// fallbacks after a failure: a directory with no `origin` remote (or no
/// repository at all) is an ordinary case, and the path-derived name is the
/// intended answer rather than a degraded one. There is nothing to report,
/// so this function keeps its infallible signature.
///
/// # Examples
///
/// ```rust,ignore
/// use camino::Utf8Path;
/// use dbar::command::RealCommandRunner;
/// use dbar::git::project_name;
///
/// let runner = RealCommandRunner::default();
/// let name = project_name(&runner, Utf8Path::new("."));
/// println!("{name}");
/// ```
pub fn project_name(runner: &dyn CommandRunner, project_dir: &Utf8Path) -> ProjectName {
    let origin = git_command(project_dir, ["remote", "get-url", "origin"]);
    if let Ok(output) = runner.run(&origin)
        && let Some(name) = parse_origin_name(&output.stdout)
    {
        return name;
    }

    if let Some(name) = name_from_worktree_path(project_dir) {
        return name;
    }

    ProjectName::new(project_dir.file_name().unwrap_or_default())
}

/// Load git status information for the given project directory.
///
/// The module-level fallback policy describes what each variant renders.
///
/// # Examples
///
/// ```rust,ignore
/// use camino::Utf8Path;
/// use dbar::command::RealCommandRunner;
/// use dbar::git::git_status;
///
/// let runner = RealCommandRunner::default();
/// let outcome = git_status(&runner, Utf8Path::new("."));
/// let _ = outcome.status();
/// ```
pub fn git_status(runner: &dyn CommandRunner, project_dir: &Utf8Path) -> GitStatusOutcome {
    match probe_repository(runner, project_dir) {
        Ok(true) => GitStatusOutcome::Available(collect_status(runner, project_dir)),
        Ok(false) => GitStatusOutcome::NotARepository,
        Err(failure) => GitStatusOutcome::Unavailable(failure),
    }
}

/// Gather every field-level probe into a single report.
fn collect_status(runner: &dyn CommandRunner, project_dir: &Utf8Path) -> GitStatusReport {
    let branch = probe_branch(runner, project_dir);
    let worktree = probe_worktree_status(runner, project_dir);
    let counts = probe_upstream_counts(runner, project_dir);

    let degradations = [branch.failure, worktree.failure, counts.failure]
        .into_iter()
        .flatten()
        .collect();
    let (dirty, staged) = worktree.value;
    let (ahead, behind) = counts.value;

    GitStatusReport {
        status: GitStatus {
            branch: branch.value,
            dirty,
            staged,
            ahead,
            behind,
            is_worktree: is_worktree_path(project_dir),
        },
        degradations,
    }
}

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

fn name_from_worktree_path(path: &Utf8Path) -> Option<ProjectName> {
    let value = path.as_str();
    let marker = ".worktrees";
    let (before, _) = value.split_once(marker)?;
    // `project/.worktrees/branch` leaves `before` as `project/`, whose last
    // `/`-separated segment is empty; trim the separator so the project name is
    // the directory that contains the marker.
    let name = before.trim_end_matches('/').rsplit('/').next()?;
    if name.is_empty() {
        None
    } else {
        Some(ProjectName::new(name.to_owned()))
    }
}

fn is_worktree_path(path: &Utf8Path) -> bool {
    let value = path.as_str();
    value.contains(".worktrees") || value.contains("/.git/worktrees/")
}

#[cfg(test)]
mod tests;
