//! Tests for the crate's run boundary, where ambient inputs are resolved.

use super::*;
use crate::status::StatusDiagnostics;
use crate::tmux::{TmuxField, TmuxProbeFailure};
use mockable::MockEnv;
use rstest::rstest;

/// An environment in which `DBAR_DIAGNOSTICS` has the given value.
fn env_with(value: Option<&str>) -> MockEnv {
    let mut env = MockEnv::new();
    let owned = value.map(std::borrow::ToOwned::to_owned);
    env.expect_string()
        .withf(|key| key == DIAGNOSTICS_ENV)
        .times(1)
        .returning(move |_| owned.clone());
    env
}

/// Any value enables diagnostics, including the empty string; only an unset
/// variable disables them.
#[rstest]
#[case::unset(None, false)]
#[case::empty(Some(""), true)]
#[case::set(Some("1"), true)]
fn diagnostics_enabled_follows_the_environment(
    #[case] value: Option<&str>,
    #[case] expected: bool,
) {
    let env = env_with(value);
    assert_eq!(diagnostics_enabled(&env), expected);
}

/// Diagnostics with one absorbed tmux failure to describe.
fn degraded_diagnostics() -> StatusDiagnostics {
    StatusDiagnostics {
        tmux: vec![TmuxProbeFailure::EmptyValue {
            field: TmuxField::Session,
        }],
        ..StatusDiagnostics::default()
    }
}

#[test]
fn diagnostic_lines_are_emitted_only_when_enabled() {
    let diagnostics = degraded_diagnostics();
    assert!(diagnostic_lines(&diagnostics, false).is_empty());

    let enabled = diagnostic_lines(&diagnostics, true);
    assert_eq!(enabled, diagnostics.describe_failures());
    assert!(!enabled.is_empty(), "the fixture must describe a failure");
}
