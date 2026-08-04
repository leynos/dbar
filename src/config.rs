//! Configuration and CLI parsing for dbar.

use camino::Utf8PathBuf;
use clap::{Parser, Subcommand};
use directories::BaseDirs;
use ortho_config::OrthoConfig;
use ortho_config::SubcmdConfigMerge;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::types::{CacheTtlSeconds, StatusPosition};

#[derive(Debug, Parser)]
#[command(author, version, about)]
/// Top-level CLI arguments for dbar.
pub struct Cli {
    /// The subcommand to execute.
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand)]
/// Available subcommands for dbar.
pub enum Commands {
    /// Render a status line segment.
    Status(StatusArgs),
    /// Install the tmux configuration snippet.
    Install(InstallArgs),
}

#[derive(Debug, Clone, Deserialize, Serialize, OrthoConfig, Default, Parser)]
#[ortho_config(prefix = "DBAR")]
// `ortho_config` derives the configuration namespace from the clap command
// name. Without an explicit name each derived `Parser` reports the package
// name, so both subcommands would share `[cmds.dbar]` and the documented
// `[cmds.status]` section and `DBAR_CMDS_STATUS_*` variables would be ignored.
#[command(name = "status")]
/// Arguments for rendering a status line.
pub struct StatusArgs {
    /// Override the project directory used for git probing.
    #[arg(long)]
    pub project_dir: Option<Utf8PathBuf>,
    /// tmux client width used for right-aligned layout.
    #[arg(long)]
    pub client_width: Option<u16>,
    /// tmux session name, supplied by tmux formats.
    #[arg(long)]
    pub session: Option<String>,
    /// tmux window index, supplied by tmux formats.
    #[arg(long)]
    pub window: Option<String>,
    /// tmux pane id, supplied by tmux formats.
    #[arg(long)]
    pub pane: Option<String>,
    /// tmux socket path, supplied by tmux formats.
    #[arg(long)]
    pub socket: Option<String>,
    /// Whether to attempt GitHub PR lookup.
    #[arg(long)]
    pub show_pr: Option<bool>,
    /// Whether to render a clock in the right-hand status segment.
    #[arg(long)]
    pub show_clock: Option<bool>,
    /// Clock format string, using chrono strftime syntax.
    #[ortho_config(default = default_clock_format())]
    #[arg(long, default_value_t = default_clock_format())]
    pub clock_format: String,
    /// Mock PR number for GitHub lookups (used in tests).
    #[arg(long)]
    pub github_mock_pr: Option<String>,
    /// Cache TTL for PR lookups, in seconds.
    #[ortho_config(default = CacheTtlSeconds::default())]
    #[arg(long, default_value_t = CacheTtlSeconds::default())]
    pub pr_cache_ttl_seconds: CacheTtlSeconds,
    /// Override the cache directory used for PR lookups.
    #[arg(long)]
    pub cache_dir: Option<Utf8PathBuf>,
}

#[derive(Debug, Clone, Deserialize, Serialize, OrthoConfig, Default, Parser)]
#[ortho_config(prefix = "DBAR")]
#[command(name = "install")]
/// Arguments for installing tmux configuration.
pub struct InstallArgs {
    /// Path to the tmux configuration file to edit.
    #[ortho_config(default = default_tmux_config_path())]
    #[arg(long)]
    pub path: Option<Utf8PathBuf>,
    /// Emit the snippet without writing it.
    #[ortho_config(default = false)]
    #[arg(long)]
    pub dry_run: bool,
    /// Install the full-width snippet with client width support.
    #[ortho_config(default = false)]
    #[arg(long)]
    pub full: bool,
    /// Where to install the status segment (left or right).
    #[ortho_config(default = StatusPosition::Left)]
    #[arg(long)]
    pub position: Option<StatusPosition>,
}

pub(crate) fn default_tmux_config_path() -> Utf8PathBuf {
    let fallback = Utf8PathBuf::from(".tmux.conf");
    let Some(base_dirs) = BaseDirs::new() else {
        return fallback;
    };
    let path = base_dirs.home_dir().join(".tmux.conf");
    Utf8PathBuf::from_path_buf(path).unwrap_or(fallback)
}

fn default_clock_format() -> String {
    "%H:%M".to_owned()
}

/// The merged command selected by the CLI.
#[derive(Debug)]
pub enum DbarCommand {
    /// Render a status line.
    Status(StatusArgs),
    /// Install the tmux snippet.
    Install(InstallArgs),
}

/// Load the CLI arguments and merge configuration defaults.
///
/// # Examples
///
/// ```rust,ignore
/// use dbar::config::load_command;
///
/// let command = load_command()?;
/// # Ok::<(), std::sync::Arc<ortho_config::OrthoError>>(())
/// ```
pub fn load_command() -> Result<DbarCommand, ConfigError> {
    load_command_from(std::env::args_os()).map_err(|err| match err {
        // Render help, version, and usage errors then exit, preserving the
        // conventional command-line behaviour and exit codes.
        ConfigError::Cli(cli) => cli.exit(),
        ConfigError::Merge(merge) => ConfigError::Merge(merge),
    })
}

/// Load a command from an explicit argument list.
///
/// Behaves like [`load_command`] but takes the arguments rather than reading
/// the process command line, so callers and tests can drive parsing without
/// touching global state. Argument errors are returned instead of exiting.
///
/// # Errors
///
/// Returns [`ConfigError::Cli`] when the arguments do not parse and
/// [`ConfigError::Merge`] when environment or configuration-file merging fails.
pub fn load_command_from<I, T>(args: I) -> Result<DbarCommand, ConfigError>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    merge_command(Cli::try_parse_from(args).map_err(Box::new)?)
}

fn merge_command(cli: Cli) -> Result<DbarCommand, ConfigError> {
    match cli.command {
        Commands::Status(args) => Ok(DbarCommand::Status(args.load_and_merge()?)),
        Commands::Install(args) => Ok(DbarCommand::Install(args.load_and_merge()?)),
    }
}

/// Errors raised while assembling a command from all configuration sources.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The command-line arguments were not valid.
    #[error(transparent)]
    Cli(#[from] Box<clap::Error>),
    /// Environment or configuration-file values could not be merged.
    #[error(transparent)]
    Merge(#[from] Arc<ortho_config::OrthoError>),
}

#[cfg(test)]
mod tests {
    //! Tests for argument parsing and configuration precedence.
    //!
    //! `ortho_config` reads `DBAR_*` variables and configuration files from the
    //! ambient environment, so environment-dependent cases serialize on a shared
    //! lock and restore every variable they touch. Argument-only cases still take
    //! the lock, because a stray `DBAR_*` value would otherwise perturb them.
    use super::*;
    use rstest::rstest;
    use std::ffi::OsString;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    /// Serializes every test that touches process-wide environment state.
    fn env_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Restores the variables it captured when dropped.
    struct EnvGuard {
        saved: Vec<(String, Option<OsString>)>,
    }

    impl EnvGuard {
        fn set(pairs: &[(&str, &str)]) -> Self {
            let saved = pairs
                .iter()
                .map(|(key, _)| ((*key).to_owned(), std::env::var_os(key)))
                .collect();
            for (key, value) in pairs {
                // SAFETY: `env_lock` serializes every mutation in this module and
                // the guard restores the previous value on drop.
                unsafe { std::env::set_var(key, value) };
            }
            Self { saved }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in &self.saved {
                // SAFETY: as above; the lock is still held by the test.
                match value {
                    Some(previous) => unsafe { std::env::set_var(key, previous) },
                    None => unsafe { std::env::remove_var(key) },
                }
            }
        }
    }

    /// Variables that would otherwise leak real user configuration into a test.
    const ISOLATING: [(&str, &str); 4] = [
        ("DBAR_CONFIG_PATH", ""),
        ("DBAR_SESSION", ""),
        ("XDG_CONFIG_HOME", "/nonexistent-dbar-test"),
        ("XDG_CONFIG_DIRS", ""),
    ];

    fn status_of(command: DbarCommand) -> StatusArgs {
        match command {
            DbarCommand::Status(args) => args,
            DbarCommand::Install(_) => panic!("expected the status subcommand"),
        }
    }

    #[rstest]
    fn documented_defaults_apply_when_nothing_overrides_them() {
        let _lock = env_lock();
        let _env = EnvGuard::set(&ISOLATING);

        let args = status_of(load_command_from(["dbar", "status"]).expect("bare status parses"));
        assert_eq!(args.clock_format, "%H:%M");
        assert_eq!(args.pr_cache_ttl_seconds, CacheTtlSeconds::default());
        assert_eq!(args.show_pr, None);
    }

    #[rstest]
    fn command_line_values_are_applied() {
        let _lock = env_lock();
        let _env = EnvGuard::set(&ISOLATING);

        let args = status_of(
            load_command_from([
                "dbar",
                "status",
                "--session",
                "demo",
                "--clock-format",
                "%H",
                "--pr-cache-ttl-seconds",
                "5",
                "--show-pr",
                "false",
            ])
            .expect("status arguments parse"),
        );
        assert_eq!(args.session.as_deref(), Some("demo"));
        assert_eq!(args.clock_format, "%H");
        assert_eq!(args.pr_cache_ttl_seconds.value(), 5);
        assert_eq!(args.show_pr, Some(false));
    }

    #[rstest]
    fn install_arguments_are_applied() {
        let _lock = env_lock();
        let _env = EnvGuard::set(&ISOLATING);

        let command = load_command_from(["dbar", "install", "--position", "right", "--full"])
            .expect("install parses");
        let DbarCommand::Install(args) = command else {
            panic!("expected the install subcommand");
        };
        assert_eq!(args.position, Some(StatusPosition::Right));
        assert!(args.full);
    }

    #[rstest]
    fn environment_values_apply_when_the_command_line_is_silent() {
        let _lock = env_lock();
        let _env = EnvGuard::set(&ISOLATING);
        let _vars = EnvGuard::set(&[("DBAR_CMDS_STATUS_SESSION", "from-env")]);

        let args = status_of(load_command_from(["dbar", "status"]).expect("status parses"));
        assert_eq!(args.session.as_deref(), Some("from-env"));
    }

    #[rstest]
    fn the_command_line_overrides_the_environment() {
        let _lock = env_lock();
        let _env = EnvGuard::set(&ISOLATING);
        let _vars = EnvGuard::set(&[("DBAR_CMDS_STATUS_SESSION", "from-env")]);

        let args = status_of(
            load_command_from(["dbar", "status", "--session", "from-cli"]).expect("status parses"),
        );
        assert_eq!(args.session.as_deref(), Some("from-cli"));
    }

    #[rstest]
    #[case::unknown_subcommand(&["dbar", "bogus"])]
    #[case::unknown_flag(&["dbar", "status", "--nope"])]
    #[case::invalid_ttl(&["dbar", "status", "--pr-cache-ttl-seconds", "abc"])]
    #[case::invalid_position(&["dbar", "install", "--position", "sideways"])]
    #[case::missing_value(&["dbar", "status", "--session"])]
    fn invalid_arguments_are_reported_rather_than_exiting(#[case] argv: &[&str]) {
        let _lock = env_lock();
        let _env = EnvGuard::set(&ISOLATING);

        let err = load_command_from(argv.iter().copied())
            .expect_err("invalid arguments must be rejected");
        assert!(
            matches!(err, ConfigError::Cli(_)),
            "expected a CLI error, got {err:?}"
        );
    }
}
