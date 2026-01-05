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
    pub position: StatusPosition,
}

fn default_tmux_config_path() -> Utf8PathBuf {
    let fallback = Utf8PathBuf::from(".tmux.conf");
    let Some(base_dirs) = BaseDirs::new() else {
        return fallback;
    };
    let path = base_dirs.home_dir().join(".tmux.conf");
    Utf8PathBuf::from_path_buf(path).unwrap_or(fallback)
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
pub fn load_command() -> Result<DbarCommand, Arc<ortho_config::OrthoError>> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Status(args) => Ok(DbarCommand::Status(args.load_and_merge()?)),
        Commands::Install(args) => Ok(DbarCommand::Install(args.load_and_merge()?)),
    }
}
