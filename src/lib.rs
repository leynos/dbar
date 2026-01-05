//! dbar status line library and CLI helpers.

mod cache;
mod command;
mod config;
mod error;
mod git;
mod install;
mod render;
mod status;
mod tmux;
mod types;

pub use crate::error::DbarError;

use crate::command::RealCommandRunner;
use crate::config::DbarCommand;
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
#[expect(clippy::print_stdout, reason = "CLI output is the intended behaviour")]
pub fn run() -> Result<(), DbarError> {
    let command = config::load_command()?;
    match command {
        DbarCommand::Status(args) => {
            let runner = RealCommandRunner;
            let clock = DefaultClock;
            let line = status::build_status_line(&args, &runner, &clock)?;
            println!("{line}");
        }
        DbarCommand::Install(args) => {
            let path = args
                .path
                .or_else(|| Some(config::default_tmux_config_path()));
            let position = args.position.unwrap_or_default();
            let outcome = install::install(path, position, args.dry_run, args.full)?;
            if outcome.dry_run {
                println!("Dry run for {}:", outcome.path);
                println!("{}", outcome.snippet);
            } else if outcome.updated {
                println!("Updated tmux config at {}", outcome.path);
                if let Some(backup) = outcome.backup_path {
                    println!("Backup written to {backup}");
                }
            } else {
                println!("tmux config already up to date at {}", outcome.path);
            }
        }
    }
    Ok(())
}
