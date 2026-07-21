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
mod tmux;
mod types;

pub use crate::error::DbarError;

use crate::command::RealCommandRunner;
use crate::config::DbarCommand;
use crate::github::{GhCliClient, GitHubClient, MockGitHubClient};
use mockable::DefaultClock;

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
    match config::load_command()? {
        DbarCommand::Status(args) => run_status(&args),
        DbarCommand::Install(args) => run_install(args),
    }
}

/// Render a status line segment and print it to stdout.
#[expect(clippy::print_stdout, reason = "CLI output is the intended behaviour")]
fn run_status(args: &config::StatusArgs) -> Result<(), DbarError> {
    let runner = RealCommandRunner;
    let clock = DefaultClock;
    let mock_client = args.github_mock_pr.as_deref().map(MockGitHubClient::new);
    let gh_client = GhCliClient::new(&runner);
    let github: &dyn GitHubClient = match mock_client.as_ref() {
        Some(client) => client,
        None => &gh_client,
    };
    let line = status::build_status_line(args, &runner, &clock, github)?;
    println!("{line}");
    Ok(())
}

/// Install the tmux snippet and report the outcome to stdout.
fn run_install(args: config::InstallArgs) -> Result<(), DbarError> {
    let path = args
        .path
        .or_else(|| Some(config::default_tmux_config_path()));
    let position = args.position.unwrap_or_default();
    let outcome = install::install(path, position, args.dry_run, args.full)?;
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
