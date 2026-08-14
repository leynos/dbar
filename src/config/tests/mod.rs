//! Shared harness for the configuration tests, split across [`arguments`] and
//! [`files`].
//!
//! `ortho_config` reads `DBAR_*` variables and configuration files from the
//! ambient environment, so environment-dependent cases serialize on a shared
//! lock and restore every variable they touch. Argument-only cases still take
//! the lock, because a stray `DBAR_*` value would otherwise perturb them.
//!
//! Note that subcommand merging does *not* honour `DBAR_CONFIG_PATH`:
//! `ortho_config` builds its candidate list from `$HOME/.dbar.toml`, the XDG
//! configuration directories, and `./.dbar.toml`. The harness therefore
//! redirects `HOME` at a temporary directory to both isolate and supply the
//! configuration file.
mod arguments;
mod files;

use super::*;
use cap_std::ambient_authority;
use cap_std::fs_utf8::Dir;
use rstest::rstest;
use std::ffi::OsString;
use std::sync::{Mutex, MutexGuard, OnceLock};
use tempfile::TempDir;

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
///
/// `HOME` is redirected as well as the XDG variables because subcommand
/// discovery consults `$HOME/.dbar.toml` directly; leaving it alone would let a
/// developer's own dotfile decide the result.
const ISOLATING: [(&str, &str); 5] = [
    ("DBAR_CONFIG_PATH", ""),
    ("DBAR_SESSION", ""),
    ("HOME", "/nonexistent-dbar-test"),
    ("XDG_CONFIG_HOME", "/nonexistent-dbar-test"),
    ("XDG_CONFIG_DIRS", ""),
];

fn status_of(command: DbarCommand) -> StatusArgs {
    match command {
        DbarCommand::Status(args) => args,
        DbarCommand::Install(_) => panic!("expected the status subcommand"),
    }
}

fn install_of(command: DbarCommand) -> InstallArgs {
    match command {
        DbarCommand::Install(args) => args,
        DbarCommand::Status(_) => panic!("expected the install subcommand"),
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
/// `DBAR_CONFIG_PATH`, so tests point `HOME` at the returned directory. The
/// developer's real configuration is never touched.
fn config_home(contents: &str) -> Result<ConfigHome, FixtureError> {
    let dir = TempDir::new()?;
    let home = Utf8PathBuf::from_path_buf(dir.path().to_path_buf())
        .map_err(|_| FixtureError::NonUtf8Path)?;
    let handle = Dir::open_ambient_dir(&home, ambient_authority())?;
    handle.write(DOTFILE, contents)?;
    Ok(ConfigHome { _dir: dir, home })
}
