//! Shared harness for the configuration tests, split across [`arguments`],
//! [`files`], and [`precedence`].
//!
//! Every case loads through [`load`], which hands `ortho_config` one injected
//! [`MapEnv`] behind both the file-discovery and the `DBAR_CMDS_*` layers, so
//! nothing reads or writes the process environment or working directory and the
//! cases run concurrently. [`isolated_env`] is the starting point: a closed map
//! that pins the working directory and names `HOME` and the XDG variables, so
//! no host configuration directory contributes. A case layers its own variables
//! on with [`MapEnv::with_var`].
//!
//! Note that subcommand merging does *not* honour `DBAR_CONFIG_PATH`:
//! `ortho_config` builds its candidate list from `$HOME/.dbar.toml`, the XDG
//! configuration directories, and `./.dbar.toml`. The harness therefore names
//! `HOME` as a temporary directory to supply the configuration file.
mod arguments;
mod files;
mod precedence;

use super::*;
use crate::types::StatusPosition;
use cap_std::ambient_authority;
use cap_std::fs_utf8::Dir;
use ortho_config::MapEnv;
use rstest::rstest;
use tempfile::TempDir;

/// A path no test creates, used for every directory the harness must keep empty.
///
/// `HOME` and the XDG directory are named as this path, and the working
/// directory is pinned to it, so a developer's own dotfile can never decide a
/// result. A path that does not exist is the strongest isolation, because a
/// lookup there finds nothing.
const NOWHERE: &str = "/nonexistent-dbar-test";

/// A closed environment that contributes nothing from the host.
///
/// The working directory is pinned, and `HOME`, `XDG_CONFIG_HOME` and
/// `XDG_CONFIG_DIRS` are named, because subcommand discovery consults
/// `$HOME/.dbar.toml`, the XDG directories and `./.dbar.toml`.
fn isolated_env() -> MapEnv {
    MapEnv::new()
        .with_current_dir(NOWHERE)
        .with_var("HOME", NOWHERE)
        .with_var("XDG_CONFIG_HOME", NOWHERE)
        .with_var("XDG_CONFIG_DIRS", NOWHERE)
}

/// The isolated environment with `HOME` named as the directory of `file`.
fn home_env(file: &ConfigHome) -> MapEnv {
    isolated_env().with_var("HOME", file.home.as_str())
}

/// Loads a command from `args` through the injected `env`.
fn load<I, T>(env: MapEnv, args: I) -> Result<DbarCommand, ConfigError>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    load_command_from_with_env(args, Arc::new(env))
}

fn status_of(command: DbarCommand) -> StatusArgs {
    match command {
        DbarCommand::Status(args) => args,
        DbarCommand::Refresh(_) | DbarCommand::Install(_) => {
            panic!("expected the status subcommand")
        }
    }
}

fn install_of(command: DbarCommand) -> InstallArgs {
    match command {
        DbarCommand::Install(args) => args,
        DbarCommand::Refresh(_) | DbarCommand::Status(_) => {
            panic!("expected the install subcommand")
        }
    }
}

/// Failures raised while preparing a temporary configuration file.
///
/// The fixture is fallible and lives outside `#[test]`, so it reports errors
/// rather than panicking; tests propagate them with `?`.
#[derive(Debug, thiserror::Error)]
enum FixtureError {
    /// The temporary directory or file could not be created.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// The temporary directory was not a valid UTF-8 path.
    #[error("the temporary directory path was not valid UTF-8")]
    NonUtf8Path,
}

/// A temporary home directory containing a `.dbar.toml` configuration file.
///
/// Dropping the [`TempDir`] removes the file, so tests must keep the value
/// bound for the duration of the load under test.
struct ConfigHome {
    _dir: TempDir,
    home: Utf8PathBuf,
}

/// The dotfile name subcommand discovery searches for in the home directory.
///
/// `ortho_config` derives it from the `DBAR` prefix, lowercased.
const DOTFILE: &str = ".dbar.toml";

/// Writes `contents` to `.dbar.toml` inside a fresh temporary home directory.
///
/// Subcommand merging searches `$HOME/.dbar.toml` rather than honouring
/// `DBAR_CONFIG_PATH`, so tests name `HOME` as the returned directory. The
/// developer's real configuration is never touched.
fn config_home(contents: &str) -> Result<ConfigHome, FixtureError> {
    let dir = TempDir::new()?;
    let home = Utf8PathBuf::from_path_buf(dir.path().to_path_buf())
        .map_err(|_| FixtureError::NonUtf8Path)?;
    let handle = Dir::open_ambient_dir(&home, ambient_authority())?;
    handle.write(DOTFILE, contents)?;
    Ok(ConfigHome { _dir: dir, home })
}
