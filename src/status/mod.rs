//! Status line assembly for dbar.
//!
//! This module is the boundary between the probes (`git`, `tmux`, `gh`, the
//! on-disk cache) and the renderer. Every probe reports a typed outcome, and
//! this module applies the documented fallback policy to turn those outcomes
//! into the status line while keeping the failures inspectable through
//! [`StatusReport::diagnostics`].
//!
//! # Fallback policy
//!
//! No failure below changes what is printed; the rendered contract is exactly
//! what dbar has always produced. The per-probe tables live with the probes:
//! see [`crate::git`], [`crate::tmux`], and [`pr`]. This module adds one rule
//! of its own: an invalid `clock_format` is *not* a degradation. It is a
//! configuration error, so it fails the whole command with a message naming
//! the offending format string rather than rendering a wrong clock.

use std::io::{self, ErrorKind};

use camino::{Utf8Path, Utf8PathBuf};
use mockable::Clock;

use crate::cache::{self, CacheLookup};
use crate::command::CommandRunner;
use crate::config::StatusArgs;
use crate::error::DbarError;
use crate::git::{self, GitProbeFailure};
use crate::github::GitHubClient;
use crate::render;
use crate::tmux::{self, TmuxContext, TmuxProbeFailure};
use crate::types::CacheTtlSeconds;

pub mod cache_key;
pub mod clock;
pub mod pr;

use cache_key::pr_cache_path;
use clock::render_clock;
use pr::{
    CacheOutcome, CacheWriteOutcome, PersistRequest, PersistSkipReason, PrLookupReport,
    PrResolution,
};

#[derive(Debug, Default)]
/// Typed diagnostics gathered while assembling one status line.
///
/// Every entry describes a failure the fallback policy absorbed. An empty
/// set of diagnostics means every probe answered cleanly.
pub struct StatusDiagnostics {
    /// Git probes that failed or returned unusable output.
    pub git: Vec<GitProbeFailure>,
    /// tmux queries that failed or returned an empty value.
    pub tmux: Vec<TmuxProbeFailure>,
    /// How the PR number was resolved, when a lookup was attempted.
    pub pr: Option<PrLookupReport>,
}

impl StatusDiagnostics {
    /// Describe every absorbed failure, one line apiece.
    ///
    /// # Examples
    ///
    /// ```text
    /// assert!(StatusDiagnostics::default().describe_failures().is_empty());
    /// ```
    pub fn describe_failures(&self) -> Vec<String> {
        let git = self.git.iter().map(ToString::to_string);
        let tmux = self.tmux.iter().map(ToString::to_string);
        let lookup = self.pr.iter().flat_map(PrLookupReport::describe_failures);
        git.chain(tmux).chain(lookup).collect()
    }
}

/// A rendered status line together with the failures behind it.
pub struct StatusReport {
    /// The tmux-ready status line to print.
    pub line: String,
    /// What degraded while assembling it.
    pub diagnostics: StatusDiagnostics,
}

/// Build a full tmux status line, reporting every absorbed probe failure.
///
/// This replaces the earlier `build_status_line`: the rendered line is
/// `StatusReport::line` and is byte-for-byte what that function returned, but
/// the failures behind a degraded line are no longer discarded.
///
/// # Examples
///
/// ```text
/// // `runner` is any `CommandRunner`; a real one spawns `git` and `gh`,
/// // so the tests inject `MockCommandRunner` instead.
/// let args = StatusArgs::default();
/// let github = GhCliClient::new(runner);
/// let clock = DefaultClock;
/// let report = build_status_report(&args, &runner, &clock, &github)?;
/// for failure in report.diagnostics.describe_failures() {
///     eprintln!("{failure}");
/// }
/// ```
///
/// # Errors
///
/// Returns an error if the project directory cannot be resolved or
/// `clock_format` is not a valid strftime format.
pub fn build_status_report(
    args: &StatusArgs,
    runner: &dyn CommandRunner,
    clock: &dyn Clock,
    github: &dyn GitHubClient,
) -> Result<StatusReport, DbarError> {
    let project_dir = resolve_project_dir(args)?;
    let project = git::project_name(runner, &project_dir);
    let git_outcome = git::git_status(runner, &project_dir);

    // No branch means no lookup: the cache key and the branch heuristic both
    // need one, so a non-repository — and equally a detached `HEAD`, which has
    // no branch to key on — simply has no PR segment. Nothing synthetic is
    // substituted here, so no cache entry can be written under an invented
    // branch name.
    let pr_report = git_outcome
        .status()
        .and_then(|status| status.branch.as_ref())
        .filter(|_| args.show_pr.unwrap_or(true))
        .map(|branch| {
            resolve_pr_number(&PrLookup {
                args,
                clock,
                github,
                project_dir: &project_dir,
                branch: branch.as_ref(),
            })
        });

    let tmux_resolution = tmux::resolve_context(
        runner,
        TmuxContext {
            session: args.session.clone(),
            window: args.window.clone(),
            pane: args.pane.clone(),
            socket: args.socket.clone(),
        },
    );
    let clock_label = render_clock(args, clock)?;

    let line = render::render_status_line(&render::RenderContext {
        project: &project.name,
        git_status: git_outcome.status(),
        pr_number: pr_report
            .as_ref()
            .and_then(|report| report.pr_number.as_ref()),
        tmux: Some(&tmux_resolution.context),
        clock: clock_label.as_deref(),
        client_width: args.client_width.map(usize::from),
    });

    // The project-name probe is a git probe like any other, so its failures
    // join the rest rather than being reported separately.
    let mut git_failures = git_outcome.into_failures();
    git_failures.extend(project.into_failures());

    Ok(StatusReport {
        line,
        diagnostics: StatusDiagnostics {
            git: git_failures,
            tmux: tmux_resolution.into_failures(),
            pr: pr_report,
        },
    })
}

/// Everything one PR lookup needs, grouped to keep the argument count down.
struct PrLookup<'a> {
    /// The status arguments, for the cache directory and TTL.
    args: &'a StatusArgs,
    /// The clock used to age cache entries.
    clock: &'a dyn Clock,
    /// The GitHub client consulted on a cache miss.
    github: &'a dyn GitHubClient,
    /// The project directory the lookup is scoped to.
    project_dir: &'a Utf8Path,
    /// The branch the lookup is scoped to.
    branch: &'a str,
}

fn resolve_project_dir(args: &StatusArgs) -> Result<Utf8PathBuf, DbarError> {
    if let Some(path) = args.project_dir.clone() {
        return Ok(path);
    }
    let current = std::env::current_dir()?;
    let path = Utf8PathBuf::from_path_buf(current)
        .map_err(|_| io::Error::new(ErrorKind::InvalidData, "current directory is not UTF-8"))?;
    Ok(path)
}

/// Resolve the PR number, performing the cache read and write here.
///
/// This is the only place that touches the cache: [`pr::decide`] states the
/// policy and asks for a write, and this boundary carries it out, so a cache
/// failure is reported rather than silently swallowed inside the policy.
fn resolve_pr_number(context: &PrLookup<'_>) -> PrLookupReport {
    match cache::resolve_cache_dir(context.args.cache_dir.clone()) {
        Ok(dir) => {
            let path = pr_cache_path(&dir, context.project_dir, context.branch);
            resolve_with_cache(context, &dir, &path)
        }
        Err(error) => resolve_without_cache(context, CacheOutcome::DirUnavailable(error)),
    }
}

/// Consult the cache, then fall through to a lookup if it did not answer.
///
/// The read itself never deletes anything. When it reports an expired entry
/// this boundary — having just decided to go upstream — calls
/// [`cache::sweep_cache_dir`] itself, so the reclamation is visible here
/// rather than hidden inside the load.
fn resolve_with_cache(context: &PrLookup<'_>, dir: &Utf8Path, path: &Utf8Path) -> PrLookupReport {
    // Absent when no layer set a TTL, so the documented default is applied
    // after merging; a clap default would shadow the lower layers.
    let ttl = context.args.pr_cache_ttl_or_default();
    match cache::load_cached_value(path, context.clock, ttl) {
        Ok(CacheLookup::Fresh(value)) => PrLookupReport {
            pr_number: pr::pr_from_cache_entry(value),
            cache: CacheOutcome::Hit,
            resolution: PrResolution::FromCache,
            write: CacheWriteOutcome::Skipped(PersistSkipReason::ServedFromCache),
        },
        Ok(CacheLookup::Missing) => lookup_and_persist(context, CacheOutcome::Miss, Some(path)),
        Ok(CacheLookup::Expired) => {
            let outcome = reclaim_expired_entries(context, dir, ttl);
            lookup_and_persist(context, outcome, Some(path))
        }
        // A corrupt or unreadable entry is treated as a miss for rendering
        // purposes, but the read failure is carried into the report.
        Err(error) => lookup_and_persist(context, CacheOutcome::ReadFailed(error), Some(path)),
    }
}

/// Reclaim expired cache entries now that a fresh lookup is unavoidable.
///
/// An expired read is a miss whatever the sweep does, so a sweep failure is
/// reported alongside the miss rather than allowed to fail the status line.
fn reclaim_expired_entries(
    context: &PrLookup<'_>,
    dir: &Utf8Path,
    ttl: CacheTtlSeconds,
) -> CacheOutcome {
    match cache::sweep_cache_dir(dir, context.clock, ttl) {
        Ok(()) => CacheOutcome::Miss,
        Err(error) => CacheOutcome::ReadFailed(error),
    }
}

/// Look up without any cache path: nothing is read and nothing is written.
fn resolve_without_cache(context: &PrLookup<'_>, cache: CacheOutcome) -> PrLookupReport {
    lookup_and_persist(context, cache, None)
}

/// Run the GitHub lookup, apply the policy, and carry out its write request.
fn lookup_and_persist(
    context: &PrLookup<'_>,
    cache: CacheOutcome,
    path: Option<&Utf8Path>,
) -> PrLookupReport {
    let lookup = context
        .github
        .pr_number(context.project_dir, context.branch);
    let decision = pr::decide(lookup, context.branch);
    let write = persist(context, path, decision.persist);
    PrLookupReport {
        pr_number: decision.pr_number,
        cache,
        resolution: decision.resolution,
        write,
    }
}

/// Carry out the policy's persistence request.
fn persist(
    context: &PrLookup<'_>,
    path: Option<&Utf8Path>,
    request: PersistRequest,
) -> CacheWriteOutcome {
    let value = match request {
        PersistRequest::Skip(reason) => return CacheWriteOutcome::Skipped(reason),
        PersistRequest::Store(value) => value,
    };
    let Some(target) = path else {
        return CacheWriteOutcome::Skipped(PersistSkipReason::CacheUnavailable);
    };
    match cache::store_cached_value(target, context.clock, value) {
        Ok(()) => CacheWriteOutcome::Stored,
        Err(error) => CacheWriteOutcome::Failed(error),
    }
}

#[cfg(test)]
mod branch_tests;
#[cfg(test)]
mod diagnostics_tests;
#[cfg(test)]
mod retention_tests;
#[cfg(test)]
mod tests;
