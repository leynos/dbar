//! Configuration and CLI parsing for dbar.
//!
//! # Why every field is optional
//!
//! Values are resolved from four layers, in ascending priority: the documented
//! defaults, the configuration file, the environment, and the command line.
//! `ortho_config` implements that order by serializing the parsed CLI struct
//! and skipping `None`, so a field the operator did not supply simply does not
//! appear in the top layer and the layers beneath it decide.
//!
//! A clap `default_value_t`, or a bare `bool` flag, breaks this: clap
//! materializes a value whether or not the flag was given, and that value is
//! indistinguishable from one the operator typed, so the command-line layer
//! shadows the environment and the configuration file. Every field is
//! therefore `Option<T>` with no clap default.
//!
//! # Where the documented defaults live
//!
//! `#[ortho_config(default = ...)]` does not populate an `Option` field: a bare
//! `dbar install` merges to `path: None` and `position: None` even though both
//! carried one, which is why [`crate::run`] has always had to fall back for
//! them by hand. The defaults are therefore applied after merging, by
//! [`StatusArgs::clock_format_or_default`],
//! [`StatusArgs::pr_cache_ttl_or_default`], [`InstallArgs::is_dry_run`], and
//! [`InstallArgs::is_full`], so that each one has a single home that consumers
//! and tests can both name.
//!
//! Booleans keep their bare-flag spelling through `num_args = 0` with
//! `default_missing_value = "true"`: `--dry-run` still takes no value, an
//! omitted flag yields `None` rather than `Some(false)`, and only a flag the
//! operator actually typed reaches the merge. `ArgAction::SetTrue` cannot be
//! used, because it yields `Some(false)` for an absent flag.

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
    // Optional so an omitted flag cannot shadow the lower layers; consumers
    // apply `DEFAULT_CLOCK_FORMAT`. See the module documentation.
    #[arg(long)]
    pub clock_format: Option<String>,
    /// Mock PR number for GitHub lookups (used in tests).
    #[arg(long, hide = true)]
    pub github_mock_pr: Option<String>,
    /// Cache TTL for PR lookups, in seconds.
    // Optional for the same reason as `clock_format`; consumers fall back to
    // `CacheTtlSeconds::default`.
    #[arg(long)]
    pub pr_cache_ttl_seconds: Option<CacheTtlSeconds>,
    /// Override the cache directory used for PR lookups.
    #[arg(long)]
    pub cache_dir: Option<Utf8PathBuf>,
}

impl StatusArgs {
    /// The clock format to render with, or `%H:%M` if no layer supplied one.
    ///
    /// # Examples
    ///
    /// ```text
    /// use dbar::config::StatusArgs;
    ///
    /// assert_eq!(StatusArgs::default().clock_format_or_default(), "%H:%M");
    /// ```
    #[must_use]
    pub fn clock_format_or_default(&self) -> &str {
        self.clock_format.as_deref().unwrap_or(DEFAULT_CLOCK_FORMAT)
    }

    /// The PR cache TTL, or [`CacheTtlSeconds::default`] if no layer supplied
    /// one.
    ///
    /// # Examples
    ///
    /// ```text
    /// use dbar::config::StatusArgs;
    /// use dbar::types::CacheTtlSeconds;
    ///
    /// let ttl = StatusArgs::default().pr_cache_ttl_or_default();
    /// assert_eq!(ttl, CacheTtlSeconds::default());
    /// ```
    #[must_use]
    pub fn pr_cache_ttl_or_default(&self) -> CacheTtlSeconds {
        self.pr_cache_ttl_seconds.unwrap_or_default()
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, OrthoConfig, Default, Parser)]
#[ortho_config(prefix = "DBAR")]
#[command(name = "install")]
/// Arguments for installing tmux configuration.
pub struct InstallArgs {
    /// Path to the tmux configuration file to edit.
    // No `#[ortho_config(default = ...)]`: it does not populate an `Option`
    // field, so `crate::run` applies `default_tmux_config_path` after merging.
    #[arg(long)]
    pub path: Option<Utf8PathBuf>,
    /// Emit the snippet without writing it.
    // A bare flag that accepts no value, yet distinguishes "absent" from
    // "false". See the module documentation.
    #[arg(long, num_args = 0, default_missing_value = "true")]
    pub dry_run: Option<bool>,
    /// Install the full-width snippet with client width support.
    // Tri-state for the same reason as `dry_run`.
    #[arg(long, num_args = 0, default_missing_value = "true")]
    pub full: Option<bool>,
    /// Where to install the status segment (left or right).
    // Defaulted after merging, as for `path`.
    #[arg(long)]
    pub position: Option<StatusPosition>,
}

impl InstallArgs {
    /// Whether the snippet should be emitted without being written.
    ///
    /// Absence means the documented default of `false`; an explicit
    /// `dry_run = false` in a lower layer means the same thing.
    ///
    /// # Examples
    ///
    /// ```text
    /// use dbar::config::InstallArgs;
    ///
    /// assert!(!InstallArgs::default().is_dry_run());
    /// ```
    #[must_use]
    pub const fn is_dry_run(&self) -> bool {
        matches!(self.dry_run, Some(true))
    }

    /// Whether the full-width snippet should be installed.
    ///
    /// # Examples
    ///
    /// ```text
    /// use dbar::config::InstallArgs;
    ///
    /// assert!(!InstallArgs::default().is_full());
    /// ```
    #[must_use]
    pub const fn is_full(&self) -> bool {
        matches!(self.full, Some(true))
    }
}

pub(crate) fn default_tmux_config_path() -> Utf8PathBuf {
    let fallback = Utf8PathBuf::from(".tmux.conf");
    let Some(base_dirs) = BaseDirs::new() else {
        return fallback;
    };
    let path = base_dirs.home_dir().join(".tmux.conf");
    Utf8PathBuf::from_path_buf(path).unwrap_or(fallback)
}

/// The clock format applied when no layer supplies one.
///
/// Held here rather than on the clap attribute so that an omitted
/// `--clock-format` cannot shadow the environment or the configuration file;
/// [`crate::status::clock`] applies it after merging.
pub(crate) const DEFAULT_CLOCK_FORMAT: &str = "%H:%M";

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
/// ```text
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
        // Every other variant is returned unchanged, so that a variant added
        // later is reported rather than silently exiting the process.
        other => other,
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
    /// `clock_format` is not a format chrono can render.
    ///
    /// Held here rather than raised as an `io::Error`, because an unrenderable
    /// format is a configuration fault, not a failed system call, and naming it
    /// as such is what lets a caller tell the two apart. The offending value is
    /// carried so the message stays actionable; no other configuration is
    /// disclosed.
    #[error("invalid clock_format {0:?}")]
    InvalidClockFormat(String),
}

#[cfg(test)]
mod tests;
