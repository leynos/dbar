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

/// Render a status line segment and print it to stdout.
#[expect(clippy::print_stdout, reason = "CLI output is the intended behaviour")]
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
    println!("{}", report.line);
    report_diagnostics(&report.diagnostics, diagnostics_enabled);
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
/// appear there regardless of the flag.
#[expect(
    clippy::print_stderr,
    reason = "opt-in operator diagnostics for silently degraded probes"
)]
fn report_diagnostics(diagnostics: &status::StatusDiagnostics, enabled: bool) {
    for failure in diagnostic_lines(diagnostics, enabled) {
        eprintln!("dbar: {failure}");
    }
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
    report_install_outcome(&outcome);
    Ok(())
}

/// Print the result of an install run to stdout.
#[expect(clippy::print_stdout, reason = "CLI output is the intended behaviour")]
fn report_install_outcome(outcome: &install::InstallOutcome) {
    if outcome.dry_run {
        println!("Dry run for {}:", outcome.path);
        println!("{}", outcome.snippet);
    } else if outcome.updated {
        println!("Updated tmux config at {}", outcome.path);
        if let Some(backup) = &outcome.backup_path {
            println!("Backup written to {backup}");
        }
    } else {
        println!("tmux config already up to date at {}", outcome.path);
    }
}
