//! dbar status line library and CLI helpers.

mod cache;
mod command;
mod config;
mod error;
mod git;
mod github;
mod install;
mod render;
mod status;
#[cfg(test)]
mod test_support;
mod tmux;
mod types;

pub use crate::error::DbarError;

use std::io::{self, Write};

use crate::command::RealCommandRunner;
use crate::config::DbarCommand;
use crate::github::{GhCliClient, GitHubClient, MockGitHubClient};
use mockable::{DefaultClock, DefaultEnv, Env};

/// Run the dbar CLI and print the requested output.
///
/// # Examples
///
/// ```no_run
/// # fn main() -> Result<(), dbar::DbarError> {
/// dbar::run()?;
/// # Ok(())
/// # }
/// ```
///
/// # Errors
///
/// Returns an error if configuration cannot be loaded, commands fail, or the
/// tmux configuration cannot be updated.
pub fn run() -> Result<(), DbarError> {
    // Resolve every ambient input at this boundary so nothing below reads the
    // process environment directly.
    let diagnostics_enabled = diagnostics_enabled(&DefaultEnv::new());
    match config::load_command()? {
        DbarCommand::Status(args) => run_status(&args, diagnostics_enabled),
        DbarCommand::Install(args) => run_install(args),
    }
}

/// Environment variable that opts into printing probe diagnostics.
///
/// The status line degrades silently by design, so the typed failures behind
/// a degraded line would otherwise be invisible to an operator. Setting this
/// variable to any value mirrors them to stderr; stdout is untouched either
/// way, so the tmux contract is unchanged.
const DIAGNOSTICS_ENV: &str = "DBAR_DIAGNOSTICS";

/// Whether [`DIAGNOSTICS_ENV`] is set to any value in `env`.
///
/// Taking the environment as a trait object keeps the decision testable: the
/// caller resolves it once and passes the answer down as a plain flag.
fn diagnostics_enabled(env: &dyn Env) -> bool {
    env.string(DIAGNOSTICS_ENV).is_some()
}

/// Write one line to `writer`, treating a closed pipe as a completed write.
///
/// `dbar status` is re-run on every tmux status refresh with stdout attached to
/// a pipe tmux is free to close the moment it has what it needs, so losing that
/// race is routine rather than a fault. A [`io::ErrorKind::BrokenPipe`] is
/// therefore reported as success: the alternative is a non-zero exit and an
/// error message for an event in which nothing actually went wrong. Every other
/// kind still propagates, so a genuinely failed write is not swallowed.
fn write_line(writer: &mut impl Write, line: &str) -> io::Result<()> {
    match writeln!(writer, "{line}") {
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        result => result,
    }
}

/// Flush `writer`, treating a closed pipe as a completed flush.
///
/// Buffered output can defer the write failure to the flush, so the same
/// tolerance [`write_line`] applies has to hold here or the broken pipe simply
/// resurfaces one call later.
fn flush_writer(writer: &mut impl Write) -> io::Result<()> {
    match writer.flush() {
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        result => result,
    }
}

/// Render a status line segment and print it to stdout.
fn run_status(args: &config::StatusArgs, diagnostics_enabled: bool) -> Result<(), DbarError> {
    let runner = RealCommandRunner;
    let clock = DefaultClock;
    let mock_client = args.github_mock_pr.as_deref().map(MockGitHubClient::new);
    let gh_client = GhCliClient::new(&runner);
    let github: &dyn GitHubClient = match mock_client.as_ref() {
        Some(client) => client,
        None => &gh_client,
    };
    let report = status::build_status_report(args, &runner, &clock, github)?;
    let mut stdout = io::stdout();
    write_line(&mut stdout, &report.line)?;
    flush_writer(&mut stdout)?;
    report_diagnostics(&mut io::stderr(), &report.diagnostics, diagnostics_enabled)?;
    Ok(())
}

/// The lines to mirror to stderr, which is none unless diagnostics are enabled.
fn diagnostic_lines(diagnostics: &status::StatusDiagnostics, enabled: bool) -> Vec<String> {
    if enabled {
        diagnostics.describe_failures()
    } else {
        Vec::new()
    }
}

/// Mirror absorbed probe failures to stderr when diagnostics are enabled.
///
/// stdout carries the status line and nothing else, so diagnostics never
/// appear there regardless of the flag; `writer` is stderr in the CLI and a
/// buffer under test.
///
/// # Errors
///
/// Returns an error if writing to `writer` fails for any reason other than a
/// closed pipe.
fn report_diagnostics(
    writer: &mut impl Write,
    diagnostics: &status::StatusDiagnostics,
    enabled: bool,
) -> io::Result<()> {
    for failure in diagnostic_lines(diagnostics, enabled) {
        write_line(writer, &format!("dbar: {failure}"))?;
    }
    flush_writer(writer)
}

#[cfg(test)]
mod tests;

/// Install the tmux snippet and report the outcome to stdout.
fn run_install(args: config::InstallArgs) -> Result<(), DbarError> {
    // Both flags are tri-state so that an omitted flag cannot shadow the
    // environment or the configuration file; absence means the documented
    // default of `false`. Read before `path` is moved out of `args`.
    let mode = install::RunMode::from_dry_run(args.is_dry_run());
    let width = install::Width::from_full(args.is_full());
    let position = args.position.unwrap_or_default();
    let path = args
        .path
        .or_else(|| Some(config::default_tmux_config_path()));
    let outcome = install::install(path, position, mode, width)?;
    report_install_outcome(&mut io::stdout(), &outcome)?;
    Ok(())
}

/// Print the result of an install run to `writer`, which is stdout in the CLI.
///
/// # Errors
///
/// Returns an error if writing to `writer` fails for any reason other than a
/// closed pipe.
fn report_install_outcome(
    writer: &mut impl Write,
    outcome: &install::InstallOutcome,
) -> io::Result<()> {
    if outcome.dry_run {
        write_line(writer, &format!("Dry run for {}:", outcome.path))?;
        write_line(writer, &outcome.snippet)?;
    } else if outcome.updated {
        write_line(writer, &format!("Updated tmux config at {}", outcome.path))?;
        if let Some(backup) = outcome.backup_path.as_ref() {
            write_line(writer, &format!("Backup written to {backup}"))?;
        }
    } else {
        write_line(
            writer,
            &format!("tmux config already up to date at {}", outcome.path),
        )?;
    }
    flush_writer(writer)
}
