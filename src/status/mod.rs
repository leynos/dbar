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

use camino::{Utf8Path, Utf8PathBuf};
use mockable::Clock;

use crate::cache::FileCacheStorage;
use crate::command::CommandRunner;
use crate::config::{RefreshArgs, StatusArgs};
use crate::error::DbarError;
use crate::git::{self, GitProbeFailure};
use crate::github::{GitHubClient, GitHubError};
use crate::render;
use crate::tmux::{self, TmuxContext, TmuxProbeFailure};
use crate::types::CacheTtlSeconds;

pub(crate) mod cache;
pub mod cache_key;
pub mod clock;
pub mod pr;

use cache::{CacheReader, CacheStorage, CacheWriter, CachedPrRead, CachedValue};
use cache_key::pr_cache_path;
use clock::render_clock;
use pr::{
    CacheFailure, CacheOutcome, CacheWriteOutcome, GitHubLookupFailure, PersistRequest,
    PersistSkipReason, PrLookupReport, PrResolution,
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

/// Dependencies used to assemble one status report.
struct StatusBuild<'a> {
    /// Parsed status settings.
    args: &'a StatusArgs,
    /// Directory whose metadata is rendered.
    project_dir: &'a Utf8Path,
    /// Command runner for git and tmux probes.
    runner: &'a dyn CommandRunner,
    /// Clock for cache expiry and the optional clock segment.
    clock: &'a dyn Clock,
    /// Read-only cache port for the status query.
    cache: &'a dyn CacheReader,
}

/// Build a full tmux status line, reporting every absorbed probe failure.
///
/// This replaces the earlier `build_status_line`: the rendered line is
/// `StatusReport::line` and is byte-for-byte what that function returned, but
/// the failures behind a degraded line are no longer discarded.
///
/// # Errors
///
/// Returns an error when `clock_format` is not a valid strftime format.
pub fn build_status_report(
    args: &StatusArgs,
    project_dir: &Utf8Path,
    runner: &dyn CommandRunner,
    clock: &dyn Clock,
) -> Result<StatusReport, DbarError> {
    build_status_report_with_cache(&StatusBuild {
        args,
        project_dir,
        runner,
        clock,
        cache: &FileCacheStorage,
    })
}

/// Build a status line with an injected read-only cache port.
fn build_status_report_with_cache(request: &StatusBuild<'_>) -> Result<StatusReport, DbarError> {
    let project = git::project_name(request.runner, request.project_dir);
    let git_outcome = git::git_status(request.runner, request.project_dir);

    // No branch means no lookup: the cache key and the branch heuristic both
    // need one, so a non-repository — and equally a detached `HEAD`, which has
    // no branch to key on — simply has no PR segment. `status` is deliberately
    // a cache-only query; `refresh` owns the live GitHub lookup and mutation.
    let pr_report = git_outcome
        .status()
        .and_then(|status| status.branch.as_ref())
        .filter(|_| request.args.show_pr.unwrap_or(true))
        .map(|branch| {
            load_cached_pr(
                &CachedPrRead {
                    args: request.args,
                    clock: request.clock,
                    project_dir: request.project_dir,
                    branch: branch.as_ref(),
                },
                request.cache,
            )
        });

    let tmux_resolution = tmux::resolve_context(
        request.runner,
        TmuxContext {
            session: request.args.session.clone(),
            window: request.args.window.clone(),
            pane: request.args.pane.clone(),
            socket: request.args.socket.clone(),
        },
    );
    let clock_label = render_clock(request.args, request.clock)?;

    let line = render::render_status_line(&render::RenderContext {
        project: &project.name,
        git_status: git_outcome.status(),
        pr_number: pr_report
            .as_ref()
            .and_then(|report| report.pr_number.as_ref()),
        tmux: Some(&tmux_resolution.context),
        clock: clock_label.as_deref(),
        client_width: request.args.client_width.map(usize::from),
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

/// Everything an explicit refresh needs, grouped to keep the argument count down.
struct PrLookup<'a> {
    /// The cache-directory override.
    cache_dir: Option<Utf8PathBuf>,
    /// The cache TTL applied after configuration merging.
    ttl: CacheTtlSeconds,
    /// The clock used to age cache entries.
    clock: &'a dyn Clock,
    /// The GitHub client consulted on a cache miss.
    github: &'a dyn GitHubClient,
    /// The project directory the lookup is scoped to.
    project_dir: &'a Utf8Path,
    /// The branch the lookup is scoped to.
    branch: &'a str,
}

/// Refresh the PR cache through the explicit GitHub and storage boundary.
///
/// This is the only path that invokes GitHub or writes the cache.
/// [`load_cached_pr`] reads cached values during `status`; this explicit
/// refresh applies [`pr::decide`]'s persistence request and reports any cache
/// failure instead of silently swallowing it inside the policy.
pub struct RefreshRequest<'a> {
    /// Parsed refresh settings.
    pub args: &'a RefreshArgs,
    /// Directory whose branch is refreshed.
    pub project_dir: &'a Utf8Path,
    /// Branch that scopes the cached value.
    pub branch: &'a str,
    /// Clock used to timestamp and age entries.
    pub clock: &'a dyn Clock,
    /// Client used only by the explicit refresh operation.
    pub github: &'a dyn GitHubClient,
}

/// Refresh one cached PR value through the explicit write boundary.
pub fn refresh_pr_cache(request: &RefreshRequest<'_>) -> PrLookupReport {
    refresh_pr_cache_with_storage(request, &FileCacheStorage)
}

/// Refresh one cached PR value through an injected read/write cache port.
fn refresh_pr_cache_with_storage(
    request: &RefreshRequest<'_>,
    cache: &dyn CacheStorage,
) -> PrLookupReport {
    let context = PrLookup {
        cache_dir: request.args.cache_dir.clone(),
        ttl: request.args.pr_cache_ttl_or_default(),
        clock: request.clock,
        github: request.github,
        project_dir: request.project_dir,
        branch: request.branch,
    };
    resolve_pr_number(&context, cache)
}

/// Resolve the PR number, performing the cache read and write here.
fn resolve_pr_number(context: &PrLookup<'_>, cache: &dyn CacheStorage) -> PrLookupReport {
    match cache.resolve_dir(context.cache_dir.clone()) {
        Ok(dir) => {
            let path = pr_cache_path(&dir, context.project_dir, context.branch);
            resolve_with_cache(context, &dir, &path, cache)
        }
        Err(error) => resolve_without_cache(context, CacheOutcome::DirUnavailable(error), cache),
    }
}

/// Consult the cache, then fall through to a lookup if it did not answer.
///
/// The read itself never deletes anything. When it reports an expired entry
/// this boundary — having just decided to go upstream — calls
/// [`cache::sweep_cache_dir`] itself, so the reclamation is visible here
/// rather than hidden inside the load.
fn resolve_with_cache(
    context: &PrLookup<'_>,
    dir: &Utf8Path,
    path: &Utf8Path,
    cache: &dyn CacheStorage,
) -> PrLookupReport {
    // Absent when no layer set a TTL, so the documented default is applied
    // after merging; a clap default would shadow the lower layers.
    let ttl = context.ttl;
    match cache.load(path, context.clock, ttl) {
        Ok(CachedValue::Fresh(value)) => PrLookupReport {
            pr_number: pr::pr_from_cache_entry(value),
            cache: CacheOutcome::Hit,
            resolution: PrResolution::FromCache,
            write: CacheWriteOutcome::Skipped(PersistSkipReason::ServedFromCache),
        },
        Ok(CachedValue::Missing) => {
            lookup_and_persist(context, CacheOutcome::Miss, Some(path), cache)
        }
        Ok(CachedValue::Expired) => {
            let outcome = reclaim_expired_entries(context, dir, ttl, cache);
            lookup_and_persist(context, outcome, Some(path), cache)
        }
        // A corrupt or unreadable entry is treated as a miss for rendering
        // purposes, but the read failure is carried into the report.
        Err(error) => {
            lookup_and_persist(context, CacheOutcome::ReadFailed(error), Some(path), cache)
        }
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
    cache: &dyn CacheWriter,
) -> CacheOutcome {
    match cache.sweep(dir, context.clock, ttl) {
        Ok(()) => CacheOutcome::Miss,
        Err(error) => CacheOutcome::ReadFailed(error),
    }
}

/// Look up without any cache path: nothing is read and nothing is written.
fn resolve_without_cache(
    context: &PrLookup<'_>,
    outcome: CacheOutcome,
    cache: &dyn CacheWriter,
) -> PrLookupReport {
    lookup_and_persist(context, outcome, None, cache)
}

/// Run the GitHub lookup, apply the policy, and carry out its write request.
fn lookup_and_persist(
    context: &PrLookup<'_>,
    cache_outcome: CacheOutcome,
    path: Option<&Utf8Path>,
    cache: &dyn CacheWriter,
) -> PrLookupReport {
    let lookup = context
        .github
        .pr_number(context.project_dir, context.branch)
        .map_err(|error| github_failure(&error));
    let decision = pr::decide(lookup, context.branch);
    let write = persist(context, path, decision.persist, cache);
    PrLookupReport {
        pr_number: decision.pr_number,
        cache: cache_outcome,
        resolution: decision.resolution,
        write,
    }
}

/// Carry out the policy's persistence request.
fn persist(
    context: &PrLookup<'_>,
    path: Option<&Utf8Path>,
    request: PersistRequest,
    cache: &dyn CacheWriter,
) -> CacheWriteOutcome {
    let value = match request {
        PersistRequest::Skip(reason) => return CacheWriteOutcome::Skipped(reason),
        PersistRequest::Store(value) => value,
    };
    let Some(target) = path else {
        return CacheWriteOutcome::Skipped(PersistSkipReason::CacheUnavailable);
    };
    match cache.store(target, context.clock, value) {
        Ok(()) => CacheWriteOutcome::Stored,
        Err(error) => CacheWriteOutcome::Failed(error),
    }
}

/// Read a PR value for the status query without invoking GitHub or mutating
/// the cache.
fn load_cached_pr(request: &CachedPrRead<'_>, cache: &dyn CacheReader) -> PrLookupReport {
    let unavailable = || PrLookupReport {
        pr_number: None,
        cache: CacheOutcome::Miss,
        resolution: PrResolution::NoPr,
        write: CacheWriteOutcome::Skipped(PersistSkipReason::StatusReadOnly),
    };
    let Ok(dir) = cache.resolve_dir(request.args.cache_dir.clone()) else {
        return PrLookupReport {
            cache: CacheOutcome::DirUnavailable(CacheFailure::DirectoryUnavailable),
            ..unavailable()
        };
    };
    let path = pr_cache_path(&dir, request.project_dir, request.branch);
    match cache.load(&path, request.clock, request.args.pr_cache_ttl_or_default()) {
        Ok(CachedValue::Fresh(value)) => PrLookupReport {
            pr_number: pr::pr_from_cache_entry(value),
            cache: CacheOutcome::Hit,
            resolution: PrResolution::FromCache,
            write: CacheWriteOutcome::Skipped(PersistSkipReason::ServedFromCache),
        },
        Ok(CachedValue::Missing | CachedValue::Expired) => unavailable(),
        Err(_) => PrLookupReport {
            cache: CacheOutcome::ReadFailed(CacheFailure::Read),
            ..unavailable()
        },
    }
}

/// Convert a GitHub adapter error at the application boundary.
const fn github_failure(_error: &GitHubError) -> GitHubLookupFailure {
    GitHubLookupFailure::Unavailable
}

#[cfg(test)]
mod branch_tests;
#[cfg(test)]
mod diagnostics_tests;
#[cfg(test)]
mod retention_tests;
#[cfg(test)]
mod tests;
