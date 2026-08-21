//! PR lookup policy.
//!
//! This module decides *what* the PR segment should show and *what*, if
//! anything, the caller should persist. It performs no I/O: the cache read and
//! write happen at the boundary in [`super`], which passes the results back in
//! and applies the [`PersistRequest`] this module returns. Keeping the policy
//! pure is what makes every failure path testable without a filesystem.
//!
//! # Fallback policy
//!
//! The rendered status line is unchanged by any failure below; only the
//! diagnosis differs.
//!
//! | Situation | Rendered result |
//! | --- | --- |
//! | Fresh cache entry holding a number ([`CacheOutcome::Hit`]) | that number, GitHub is not consulted |
//! | Fresh cache entry holding the empty string | no PR segment, GitHub is not consulted |
//! | No entry or an expired entry ([`CacheOutcome::Miss`]) | whatever the GitHub lookup resolves to |
//! | Cache directory unresolvable ([`CacheOutcome::DirUnavailable`]) | as for a miss; nothing is read or written |
//! | Cache read failed ([`CacheOutcome::ReadFailed`]) | as for a miss; the lookup proceeds |
//! | GitHub returned a number ([`PrResolution::GitHub`]) | that number, and it is cached |
//! | GitHub returned no PR ([`PrResolution::BranchFallback`], [`PrResolution::NoPr`]) | the branch-derived number if the branch names one, otherwise no PR segment; either way the result is cached |
//! | GitHub lookup failed ([`PrResolution::LookupFailed`]) | the branch-derived number if the branch names one, otherwise no PR segment; **nothing is cached**, so a transient failure cannot poison the value for a whole TTL |
//! | Cache write failed ([`CacheWriteOutcome::Failed`]) | the number already resolved; only the next invocation pays for the missing entry |

use std::fmt;

use crate::cache::CacheError;
use crate::github::GitHubError;
use crate::types::PrNumber;

#[derive(Debug)]
/// What the on-disk cache contributed to a PR lookup.
pub enum CacheOutcome {
    /// A fresh entry answered the lookup, so GitHub was not consulted.
    Hit,
    /// No entry existed, or the entry had expired.
    Miss,
    /// The cache directory could not be resolved, so no entry was consulted
    /// and none can be written.
    DirUnavailable(CacheError),
    /// Reading the entry failed, so the lookup proceeded as for a miss.
    ReadFailed(CacheError),
}

#[derive(Debug)]
/// What became of the cache write the policy asked for.
pub enum CacheWriteOutcome {
    /// No write was requested.
    Skipped(PersistSkipReason),
    /// The value was written.
    Stored,
    /// The write was attempted and failed. The rendered value is unaffected.
    Failed(CacheError),
}

impl fmt::Display for CacheWriteOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Skipped(reason) => write!(f, "not written ({reason})"),
            Self::Stored => f.write_str("written"),
            Self::Failed(error) => write!(f, "write failed: {error}"),
        }
    }
}

#[derive(Debug)]
/// How the PR number was resolved once the cache had been consulted.
pub enum PrResolution {
    /// A fresh cache entry supplied the answer.
    FromCache,
    /// The GitHub lookup returned a PR number.
    GitHub,
    /// GitHub reported no PR, and the branch name supplied one.
    BranchFallback,
    /// GitHub reported no PR and the branch name did not name one either.
    NoPr,
    /// The GitHub lookup failed. The branch name was consulted instead, so
    /// the report may still carry a PR number.
    LookupFailed(GitHubError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Why the policy declined to persist a value.
pub enum PersistSkipReason {
    /// The value came straight from the cache; rewriting it would refresh the
    /// TTL of data that was never re-verified.
    ServedFromCache,
    /// The lookup failed, so the value is a guess derived from the branch
    /// name and must not be cached for the whole TTL.
    LookupFailed,
    /// No cache path was available, so there is nowhere to write.
    CacheUnavailable,
}

impl fmt::Display for PersistSkipReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::ServedFromCache => "served from the cache",
            Self::LookupFailed => "the lookup failed",
            Self::CacheUnavailable => "the cache is unavailable",
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
/// What the policy needs the caller's cache boundary to do.
pub enum PersistRequest {
    /// Write this value. The empty string records "this branch has no PR",
    /// which is worth caching so the next invocation skips the lookup.
    Store(String),
    /// Write nothing, for the stated reason.
    Skip(PersistSkipReason),
}

#[derive(Debug)]
/// The policy's decision for a single lookup.
pub struct PrDecision {
    /// The PR number to render, if any.
    pub pr_number: Option<PrNumber>,
    /// How that number was arrived at.
    pub resolution: PrResolution,
    /// What the caller should persist.
    pub persist: PersistRequest,
}

#[derive(Debug)]
/// The resolved PR number and the full typed record of how it was obtained.
pub struct PrLookupReport {
    /// The PR number to render, if any.
    pub pr_number: Option<PrNumber>,
    /// What the cache contributed.
    pub cache: CacheOutcome,
    /// How the value was resolved.
    pub resolution: PrResolution,
    /// What became of the cache write.
    pub write: CacheWriteOutcome,
}

impl PrLookupReport {
    /// Describe every failure this lookup absorbed, one line apiece.
    ///
    /// An entirely healthy lookup describes nothing.
    ///
    /// # Examples
    ///
    /// ```text
    ///     PrLookupReport, PrResolution};
    ///
    /// let report = PrLookupReport {
    ///     pr_number: None,
    ///     cache: CacheOutcome::Miss,
    ///     resolution: PrResolution::NoPr,
    ///     write: CacheWriteOutcome::Stored,
    /// };
    /// assert!(report.describe_failures().is_empty());
    /// ```
    pub fn describe_failures(&self) -> Vec<String> {
        let cache = match &self.cache {
            CacheOutcome::Hit | CacheOutcome::Miss => None,
            CacheOutcome::DirUnavailable(error) => {
                Some(format!("PR cache directory unavailable: {error}"))
            }
            CacheOutcome::ReadFailed(error) => Some(format!("PR cache read failed: {error}")),
        };
        let resolution = match &self.resolution {
            PrResolution::FromCache
            | PrResolution::GitHub
            | PrResolution::BranchFallback
            | PrResolution::NoPr => None,
            PrResolution::LookupFailed(error) => Some(format!("PR lookup failed: {error}")),
        };
        let write = match &self.write {
            CacheWriteOutcome::Skipped(_) | CacheWriteOutcome::Stored => None,
            CacheWriteOutcome::Failed(error) => Some(format!("PR cache write failed: {error}")),
        };
        [cache, resolution, write].into_iter().flatten().collect()
    }
}

/// Interpret a fresh cache entry.
///
/// The empty string is the recorded form of "this branch has no PR", so it
/// resolves to `None` rather than to a PR numbered with an empty string.
///
/// # Examples
///
/// ```text
/// assert!(pr_from_cache_entry(String::new()).is_none());
/// assert!(pr_from_cache_entry("42".to_owned()).is_some());
/// ```
pub fn pr_from_cache_entry(value: String) -> Option<PrNumber> {
    if value.is_empty() {
        None
    } else {
        Some(PrNumber::new(value))
    }
}

/// Decide the PR number and the persistence request from a completed lookup.
///
/// This function is pure: it neither reads nor writes the cache, and it never
/// calls GitHub. The caller supplies the lookup's `Result` and applies the
/// returned [`PersistRequest`].
///
/// # Examples
///
/// ```text
/// let decision = decide(Ok(None), "pr/7");
/// assert!(matches!(decision.resolution, PrResolution::BranchFallback));
/// assert_eq!(decision.persist, PersistRequest::Store("7".to_owned()));
/// ```
pub fn decide(lookup: Result<Option<PrNumber>, GitHubError>, branch: &str) -> PrDecision {
    match lookup {
        Ok(Some(pr_number)) => PrDecision {
            persist: PersistRequest::Store(pr_number.to_string()),
            pr_number: Some(pr_number),
            resolution: PrResolution::GitHub,
        },
        Ok(None) => decide_without_a_pr(branch),
        // A failed lookup (network or rate limit) must not poison the cache
        // with a fallback value for the whole TTL, so nothing is persisted.
        Err(error) => PrDecision {
            pr_number: pr_from_branch(branch),
            resolution: PrResolution::LookupFailed(error),
            persist: PersistRequest::Skip(PersistSkipReason::LookupFailed),
        },
    }
}

/// Decide what to render and persist when GitHub reported no PR.
fn decide_without_a_pr(branch: &str) -> PrDecision {
    let pr_number = pr_from_branch(branch);
    let persist = PersistRequest::Store(
        pr_number
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default(),
    );
    let resolution = if pr_number.is_some() {
        PrResolution::BranchFallback
    } else {
        PrResolution::NoPr
    };
    PrDecision {
        pr_number,
        resolution,
        persist,
    }
}

/// Derive a PR number from a `pr/<n>`-style branch name.
///
/// # Examples
///
/// ```text
/// assert!(pr_from_branch("pull-12").is_some());
/// assert!(pr_from_branch("feature/login").is_none());
/// ```
pub fn pr_from_branch(branch: &str) -> Option<PrNumber> {
    let trimmed = branch.trim();
    let stripped = trimmed
        .strip_prefix("pr/")
        .or_else(|| trimmed.strip_prefix("pr-"))
        .or_else(|| trimmed.strip_prefix("pull/"))
        .or_else(|| trimmed.strip_prefix("pull-"))?;
    if stripped.is_empty() || !stripped.chars().all(|ch| ch.is_ascii_digit()) {
        None
    } else {
        Some(PrNumber::new(stripped.to_owned()))
    }
}

#[cfg(test)]
mod tests;
