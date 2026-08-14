//! Tests for the pure PR policy: branch parsing, cache-entry decoding,
//! and the decision (including persistence) for every lookup outcome.

use super::*;
use crate::command::CommandError;
use rstest::rstest;

/// A stand-in for a failed `gh` invocation.
fn lookup_failure() -> GitHubError {
    GitHubError::Command(CommandError::NonZero {
        status: Some(1),
        stderr: "gh failed".to_owned(),
    })
}

/// Render a decision's PR number for comparison.
fn rendered(decision: &PrDecision) -> Option<String> {
    decision.pr_number.as_ref().map(ToString::to_string)
}

#[rstest]
#[case::slash_prefix("pr/7", Some("7"))]
#[case::dash_prefix("pr-7", Some("7"))]
#[case::pull_slash_prefix("pull/12", Some("12"))]
#[case::pull_dash_prefix("pull-12", Some("12"))]
#[case::leading_whitespace("  pr/7", Some("7"))]
#[case::trailing_whitespace("pr/7\n", Some("7"))]
#[case::multi_digit("pr/1234", Some("1234"))]
#[case::non_numeric_remainder("pr/feature", None)]
#[case::partly_numeric_remainder("pr/7a", None)]
#[case::empty_remainder("pr/", None)]
#[case::empty_dash_remainder("pull-", None)]
#[case::unrelated_branch("feature/login", None)]
#[case::bare_prefix("pr", None)]
#[case::empty_branch("", None)]
fn pr_from_branch_parses_accepted_prefixes(#[case] branch: &str, #[case] expected: Option<&str>) {
    let parsed = pr_from_branch(branch);
    assert_eq!(parsed.map(|pr| pr.to_string()).as_deref(), expected);
}

#[rstest]
#[case::empty("", None)]
#[case::number("42", Some("42"))]
fn pr_from_cache_entry_decodes_the_recorded_value(
    #[case] value: &str,
    #[case] expected: Option<&str>,
) {
    let parsed = pr_from_cache_entry(value.to_owned());
    assert_eq!(parsed.map(|pr| pr.to_string()).as_deref(), expected);
}

#[rstest]
fn a_successful_lookup_is_rendered_and_cached() {
    let decision = decide(Ok(Some(PrNumber::new("42"))), "feature/login");
    assert_eq!(rendered(&decision).as_deref(), Some("42"));
    assert!(matches!(decision.resolution, PrResolution::GitHub));
    assert_eq!(decision.persist, PersistRequest::Store("42".to_owned()));
}

#[rstest]
fn no_pr_falls_back_to_the_branch_name_and_is_cached() {
    let decision = decide(Ok(None), "pr/7");
    assert_eq!(rendered(&decision).as_deref(), Some("7"));
    assert!(matches!(decision.resolution, PrResolution::BranchFallback));
    assert_eq!(decision.persist, PersistRequest::Store("7".to_owned()));
}

#[rstest]
fn no_pr_at_all_caches_the_empty_marker() {
    let decision = decide(Ok(None), "feature/login");
    assert!(decision.pr_number.is_none());
    assert!(matches!(decision.resolution, PrResolution::NoPr));
    assert_eq!(decision.persist, PersistRequest::Store(String::new()));
}

#[rstest]
fn a_failed_lookup_falls_back_without_caching() {
    let decision = decide(Err(lookup_failure()), "pr/7");
    // The rendered contract is unchanged: the branch fallback still wins.
    assert_eq!(rendered(&decision).as_deref(), Some("7"));
    assert!(matches!(decision.resolution, PrResolution::LookupFailed(_)));
    assert_eq!(
        decision.persist,
        PersistRequest::Skip(PersistSkipReason::LookupFailed)
    );
}

#[rstest]
fn a_failed_lookup_without_a_usable_branch_renders_no_pr() {
    let decision = decide(Err(lookup_failure()), "feature/login");
    assert!(decision.pr_number.is_none());
    assert_eq!(
        decision.persist,
        PersistRequest::Skip(PersistSkipReason::LookupFailed)
    );
}

#[rstest]
fn a_healthy_report_describes_no_failures() {
    let report = PrLookupReport {
        pr_number: None,
        cache: CacheOutcome::Miss,
        resolution: PrResolution::NoPr,
        write: CacheWriteOutcome::Stored,
    };
    assert!(report.describe_failures().is_empty());
}

#[rstest]
fn a_degraded_report_describes_every_failure() {
    let report = PrLookupReport {
        pr_number: None,
        cache: CacheOutcome::ReadFailed(CacheError::MissingFileName),
        resolution: PrResolution::LookupFailed(lookup_failure()),
        write: CacheWriteOutcome::Failed(CacheError::MissingBaseDir),
    };
    let described = report.describe_failures();
    assert_eq!(described.len(), 3);
    assert!(described.iter().any(|line| line.contains("cache read")));
    assert!(described.iter().any(|line| line.contains("lookup failed")));
    assert!(described.iter().any(|line| line.contains("cache write")));
}

#[rstest]
#[case::served_from_cache(PersistSkipReason::ServedFromCache, "served from the cache")]
#[case::lookup_failed(PersistSkipReason::LookupFailed, "the lookup failed")]
#[case::cache_unavailable(PersistSkipReason::CacheUnavailable, "the cache is unavailable")]
fn a_skipped_write_describes_its_reason(#[case] reason: PersistSkipReason, #[case] expected: &str) {
    let outcome = CacheWriteOutcome::Skipped(reason);
    assert_eq!(outcome.to_string(), format!("not written ({expected})"));
}

#[rstest]
fn a_completed_write_describes_itself() {
    assert_eq!(CacheWriteOutcome::Stored.to_string(), "written");
    let failed = CacheWriteOutcome::Failed(CacheError::MissingBaseDir);
    assert!(failed.to_string().starts_with("write failed:"));
}
