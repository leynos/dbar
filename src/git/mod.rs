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
//! | Branch probe failed | no branch, so the renderer prints `detached` |
//! | Branch probe returned an empty name (a genuinely detached `HEAD`) | no branch, so the renderer prints `detached`; no degradation recorded |
//! | Worktree-status probe failed | neither dirty nor staged |
//! | A porcelain line was too short to classify | that line is ignored; the rest still count |
//! | Upstream-count probe failed or was unparseable | ahead and behind both read zero |
//! | Origin-URL probe exited non-zero (git's answer for "no `origin` remote", including in a plain directory) | the path-derived project name; no degradation recorded |
//! | Origin-URL probe could not be run at all, or answered with a URL naming nothing | the path-derived project name, with the failure recorded on [`ProjectNameOutcome`] |
//!
//! # Trust
//!
//! The probed directory is untrusted: it is wherever the user's shell happens
//! to be, so a repository they merely `cd`-ed into can carry a `.git/config`
//! that names commands for git to run. Every probe is therefore built by one
//! spec builder that disables those keys; see `probes::HARDENING_ARGS` for
//! which, and for why none of them changes what is reported.

use std::fmt;

use camino::Utf8Path;
use thiserror::Error;

use crate::command::{CommandError, CommandRunner};
use crate::types::{AheadCount, BehindCount, BranchName, ProjectName};

mod probes;

/// The spec builder every probe goes through, re-exported so that tests
/// elsewhere in the crate can construct the exact spec a probe produces
/// instead of hand-assembling one that would drift from the hardening.
#[cfg(test)]
pub(crate) use probes::git_command;
use probes::{
    probe_branch, probe_origin_name, probe_repository, probe_upstream_counts, probe_worktree_status,
};

#[derive(Debug, Clone)]
/// Snapshot of git status metadata for rendering.
pub struct GitStatus {
    /// The current branch, or `None` when `HEAD` is detached.
    ///
    /// `None` is not a fallback: git reports no current branch both for a
    /// detached `HEAD` and when the branch probe failed, and the renderer —
    /// not this module — decides what label stands in for it. Keeping it
    /// absent stops a synthetic name reaching the PR lookup or the cache key,
    /// where a real branch called `detached` would be indistinguishable.
    pub branch: Option<BranchName>,
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
    /// `git remote get-url origin`.
    OriginUrl,
}

impl GitProbe {
    /// The `git` sub-command this probe runs, for diagnostics.
    const fn command(self) -> &'static str {
        match self {
            Self::Repository => "rev-parse --is-inside-work-tree",
            Self::Branch => "branch --show-current",
            Self::WorktreeStatus => "status --porcelain",
            Self::UpstreamCounts => "rev-list --left-right --count @{upstream}...HEAD",
            Self::OriginUrl => "remote get-url origin",
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
    /// ```text
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
    /// ```text
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

#[derive(Debug)]
/// A resolved project name together with any failure absorbed to reach it.
///
/// Most of the fallback chain is *heuristic* rather than degraded: a
/// repository with no `origin` remote, or a plain directory, is an ordinary
/// case that git answers with a non-zero exit, and the path-derived name is
/// the intended answer. What this type makes visible is the case that is not
/// ordinary — git could not be run at all, or answered with a URL naming
/// nothing — which was previously indistinguishable from a clean answer.
pub struct ProjectNameOutcome {
    /// The name to render.
    pub name: ProjectName,
    /// The failure the fallback absorbed, if the probe genuinely failed.
    pub failure: Option<GitProbeFailure>,
}

impl ProjectNameOutcome {
    /// Consume the outcome, returning every failure it recorded.
    ///
    /// # Examples
    ///
    /// ```text
    /// // `runner` is any `CommandRunner`; a real one spawns `git`, so the
    /// // tests inject `MockCommandRunner`.
    /// let outcome = project_name(&runner, Utf8Path::new("."));
    /// let _ = outcome.into_failures();
    /// ```
    pub fn into_failures(self) -> Vec<GitProbeFailure> {
        self.failure.into_iter().collect()
    }
}

/// Resolve the project name using git metadata and directory heuristics.
///
/// The module-level fallback policy describes which failures are recorded.
/// The rendered name is unchanged by any of them: the heuristics run in the
/// same order and produce the same answer as before.
///
/// # Examples
///
/// ```text
/// // `runner` is any `CommandRunner`; a real one spawns `git`, so the
/// // tests inject `MockCommandRunner`.
/// let outcome = project_name(&runner, Utf8Path::new("."));
/// println!("{}", outcome.name);
/// ```
pub fn project_name(runner: &dyn CommandRunner, project_dir: &Utf8Path) -> ProjectNameOutcome {
    let origin = probe_origin_name(runner, project_dir);
    let name = origin
        .value
        .or_else(|| name_from_worktree_path(project_dir))
        .unwrap_or_else(|| ProjectName::new(project_dir.file_name().unwrap_or_default()));
    ProjectNameOutcome {
        name,
        failure: origin.failure,
    }
}

/// Load git status information for the given project directory.
///
/// The module-level fallback policy describes what each variant renders.
///
/// # Examples
///
/// ```text
/// // `runner` is any `CommandRunner`; a real one spawns `git`, so the
/// // tests inject `MockCommandRunner`.
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

// The fixture arms git with a POSIX shell command and an executable-bit hook,
// so the vectors it reproduces only exist on unix.
#[cfg(all(test, unix))]
mod hardening_tests;
#[cfg(test)]
mod tests;
