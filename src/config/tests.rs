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
