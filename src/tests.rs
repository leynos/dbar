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

/// A writer whose every operation fails with a fixed error kind.
struct FailingWriter {
    kind: io::ErrorKind,
}

impl Write for FailingWriter {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(self.kind, "write refused"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Err(io::Error::new(self.kind, "flush refused"))
    }
}

/// A closed pipe is the routine end of a tmux refresh, so it is not an error.
#[rstest]
#[case::broken_pipe(io::ErrorKind::BrokenPipe, true)]
#[case::other(io::ErrorKind::PermissionDenied, false)]
fn write_line_tolerates_only_a_closed_pipe(
    #[case] kind: io::ErrorKind,
    #[case] expect_success: bool,
) {
    let mut writer = FailingWriter { kind };
    assert_eq!(write_line(&mut writer, "line").is_ok(), expect_success);
    assert_eq!(flush_writer(&mut writer).is_ok(), expect_success);
}

#[test]
fn write_line_appends_a_newline() {
    let mut buffer = Vec::new();
    write_line(&mut buffer, "hello").expect("writing to a buffer cannot fail");
    assert_eq!(buffer, b"hello\n");
}

#[test]
fn report_diagnostics_writes_only_when_enabled() {
    let diagnostics = degraded_diagnostics();

    let mut quiet = Vec::new();
    report_diagnostics(&mut quiet, &diagnostics, false).expect("buffered write cannot fail");
    assert!(quiet.is_empty());

    let mut loud = Vec::new();
    report_diagnostics(&mut loud, &diagnostics, true).expect("buffered write cannot fail");
    let text = String::from_utf8(loud).expect("diagnostics are utf8");
    let mut expected = String::new();
    for failure in diagnostics.describe_failures() {
        expected.push_str("dbar: ");
        expected.push_str(&failure);
        expected.push('\n');
    }
    assert_eq!(text, expected);
}

/// An outcome with the given flags and a fixed path and snippet.
fn outcome(is_dry_run: bool, is_updated: bool, backup: Option<&str>) -> install::InstallOutcome {
    install::InstallOutcome {
        path: camino::Utf8PathBuf::from("/tmp/tmux.conf"),
        backup_path: backup.map(camino::Utf8PathBuf::from),
        is_updated,
        is_dry_run,
        snippet: "# dbar: begin\n# dbar: end\n".to_owned(),
    }
}

#[rstest]
#[case::dry_run(
    outcome(true, false, None),
    "Dry run for /tmp/tmux.conf:\n# dbar: begin\n# dbar: end\n\n"
)]
#[case::updated(outcome(false, true, None), "Updated tmux config at /tmp/tmux.conf\n")]
#[case::updated_with_backup(
    outcome(false, true, Some("/tmp/tmux.conf.bak")),
    "Updated tmux config at /tmp/tmux.conf\nBackup written to /tmp/tmux.conf.bak\n"
)]
#[case::unchanged(
    outcome(false, false, None),
    "tmux config already up to date at /tmp/tmux.conf\n"
)]
fn report_install_outcome_reports_each_case(
    #[case] outcome: install::InstallOutcome,
    #[case] expected: &str,
) {
    let mut buffer = Vec::new();
    report_install_outcome(&mut buffer, &outcome).expect("buffered write cannot fail");
    assert_eq!(String::from_utf8(buffer).expect("output is utf8"), expected);
}

/// Neither reporter turns a closed status-bar pipe into a failure.
#[test]
fn reporters_tolerate_a_closed_pipe() {
    let mut writer = FailingWriter {
        kind: io::ErrorKind::BrokenPipe,
    };
    report_diagnostics(&mut writer, &degraded_diagnostics(), true)
        .expect("a closed pipe is not an error");
    report_install_outcome(&mut writer, &outcome(true, false, None))
        .expect("a closed pipe is not an error");
}
