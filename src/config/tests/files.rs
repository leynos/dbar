//! Coverage for configuration-file loading and layer precedence.
//!
//! Each case writes a `.dbar.toml` into a temporary home directory, so the
//! developer's own configuration is never read.
use super::*;

/// A `status` section whose values differ from the defaults and every other
/// layer, so a test cannot pass by coincidence.
///
/// `clock_format` and `pr_cache_ttl_seconds` are deliberately absent: both
/// carry a clap `default_value_t`, so the command-line layer always supplies a
/// value for them and no file entry can win. See
/// `clap_defaults_shadow_configuration_file_values`, which pins that behaviour.
const FILE_CONTENTS: &str = concat!(
    "[cmds.status]\n",
    "session = \"from-file\"\n",
    "github_mock_pr = \"pr-from-file\"\n",
);

#[rstest]
#[expect(
    clippy::panic_in_result_fn,
    reason = "the test returns `Result` to propagate the fallible fixture with `?`; assertions remain the idiomatic failure mechanism"
)]
fn configuration_file_values_are_loaded() -> Result<(), FixtureError> {
    let _lock = env_lock();
    let file = config_home(FILE_CONTENTS)?;
    let _env = EnvGuard::set(&ISOLATING);
    let _home = EnvGuard::set(&[("HOME", file.home.as_str())]);

    let args = status_of(load_command_from(["dbar", "status"]).expect("status parses"));
    assert_eq!(args.session.as_deref(), Some("from-file"));
    assert_eq!(args.github_mock_pr.as_deref(), Some("pr-from-file"));
    Ok(())
}

#[rstest]
#[expect(
    clippy::panic_in_result_fn,
    reason = "the test returns `Result` to propagate the fallible fixture with `?`; assertions remain the idiomatic failure mechanism"
)]
fn the_configuration_file_overrides_the_documented_defaults() -> Result<(), FixtureError> {
    let _lock = env_lock();
    let file = config_home(concat!(
        "[cmds.install]\n",
        "position = \"right\"\n",
        "path = \"/tmp/from-file.tmux.conf\"\n",
    ))?;
    let _env = EnvGuard::set(&ISOLATING);
    let _home = EnvGuard::set(&[("HOME", file.home.as_str())]);

    // The documented defaults are `left` and the home directory's `.tmux.conf`;
    // both differ from the file's values, so the file must be what won.
    let args = install_of(load_command_from(["dbar", "install"]).expect("install parses"));
    assert_eq!(args.position, Some(StatusPosition::Right));
    assert_eq!(
        args.path.as_deref().map(camino::Utf8Path::as_str),
        Some("/tmp/from-file.tmux.conf")
    );
    assert_ne!(args.path, Some(default_tmux_config_path()));
    Ok(())
}

#[rstest]
#[expect(
    clippy::panic_in_result_fn,
    reason = "the test returns `Result` to propagate the fallible fixture with `?`; assertions remain the idiomatic failure mechanism"
)]
fn the_environment_overrides_the_configuration_file() -> Result<(), FixtureError> {
    let _lock = env_lock();
    let file = config_home(FILE_CONTENTS)?;
    let _env = EnvGuard::set(&ISOLATING);
    let _home = EnvGuard::set(&[
        ("HOME", file.home.as_str()),
        ("DBAR_CMDS_STATUS_SESSION", "from-env"),
    ]);

    let args = status_of(load_command_from(["dbar", "status"]).expect("status parses"));
    assert_eq!(args.session.as_deref(), Some("from-env"));
    // The file still supplies the values the environment leaves alone, proving
    // the file layer was loaded rather than skipped.
    assert_eq!(args.github_mock_pr.as_deref(), Some("pr-from-file"));
    Ok(())
}

#[rstest]
#[expect(
    clippy::panic_in_result_fn,
    reason = "the test returns `Result` to propagate the fallible fixture with `?`; assertions remain the idiomatic failure mechanism"
)]
fn the_command_line_overrides_the_configuration_file_and_environment() -> Result<(), FixtureError> {
    let _lock = env_lock();
    let file = config_home(FILE_CONTENTS)?;
    let _env = EnvGuard::set(&ISOLATING);
    let _home = EnvGuard::set(&[
        ("HOME", file.home.as_str()),
        ("DBAR_CMDS_STATUS_SESSION", "from-env"),
    ]);

    let args = status_of(
        load_command_from(["dbar", "status", "--session", "from-cli"]).expect("status parses"),
    );
    assert_eq!(args.session.as_deref(), Some("from-cli"));
    Ok(())
}

#[rstest]
#[expect(
    clippy::panic_in_result_fn,
    reason = "the test returns `Result` to propagate the fallible fixture with `?`; assertions remain the idiomatic failure mechanism"
)]
fn malformed_configuration_files_are_reported_rather_than_exiting() -> Result<(), FixtureError> {
    let _lock = env_lock();
    let file = config_home("[cmds.status\nsession = \"unterminated\n")?;
    let _env = EnvGuard::set(&ISOLATING);
    let _home = EnvGuard::set(&[("HOME", file.home.as_str())]);

    let err = load_command_from(["dbar", "status"]).expect_err("a malformed file must be rejected");
    assert!(
        matches!(err, ConfigError::Merge(_)),
        "expected a merge error, got {err:?}"
    );
    Ok(())
}

#[rstest]
fn install_environment_values_apply_when_the_command_line_is_silent() {
    let _lock = env_lock();
    let _env = EnvGuard::set(&ISOLATING);
    let _vars = EnvGuard::set(&[
        ("DBAR_CMDS_INSTALL_POSITION", "right"),
        ("DBAR_CMDS_INSTALL_PATH", "/tmp/from-env.tmux.conf"),
    ]);

    let args = install_of(load_command_from(["dbar", "install"]).expect("install parses"));
    assert_eq!(args.position, Some(StatusPosition::Right));
    assert_eq!(
        args.path.as_deref().map(camino::Utf8Path::as_str),
        Some("/tmp/from-env.tmux.conf")
    );
}

#[rstest]
#[expect(
    clippy::panic_in_result_fn,
    reason = "the test returns `Result` to propagate the fallible fixture with `?`; assertions remain the idiomatic failure mechanism"
)]
fn install_configuration_file_values_are_loaded() -> Result<(), FixtureError> {
    let _lock = env_lock();
    let file = config_home(concat!(
        "[cmds.install]\n",
        "position = \"right\"\n",
        "path = \"/tmp/from-file.tmux.conf\"\n",
    ))?;
    let _env = EnvGuard::set(&ISOLATING);
    let _home = EnvGuard::set(&[("HOME", file.home.as_str())]);

    let args = install_of(load_command_from(["dbar", "install"]).expect("install parses"));
    assert_eq!(args.position, Some(StatusPosition::Right));
    assert_eq!(
        args.path.as_deref().map(camino::Utf8Path::as_str),
        Some("/tmp/from-file.tmux.conf")
    );
    Ok(())
}

#[rstest]
#[expect(
    clippy::panic_in_result_fn,
    reason = "the test returns `Result` to propagate the fallible fixture with `?`; assertions remain the idiomatic failure mechanism"
)]
fn clap_defaults_shadow_configuration_file_values() -> Result<(), FixtureError> {
    let _lock = env_lock();
    let file = config_home(concat!(
        "[cmds.status]\n",
        "clock_format = \"%d %b\"\n",
        "pr_cache_ttl_seconds = 11\n",
    ))?;
    let _env = EnvGuard::set(&ISOLATING);
    let _home = EnvGuard::set(&[("HOME", file.home.as_str())]);

    let args = status_of(load_command_from(["dbar", "status"]).expect("status parses"));
    // Known deviation from the documented `defaults < file < env < CLI` order:
    // `clock_format` and `pr_cache_ttl_seconds` declare a clap `default_value_t`,
    // so clap materializes a value even when the flag is absent and the
    // command-line layer shadows the file. Only fields whose defaults come
    // solely from `#[ortho_config(default = ...)]` are overridable by a file.
    assert_eq!(args.clock_format, "%H:%M");
    assert_eq!(args.pr_cache_ttl_seconds, CacheTtlSeconds::default());
    Ok(())
}

#[rstest]
#[expect(
    clippy::panic_in_result_fn,
    reason = "the test returns `Result` to propagate the fallible fixture with `?`; assertions remain the idiomatic failure mechanism"
)]
fn install_boolean_flags_are_not_settable_from_a_configuration_file() -> Result<(), FixtureError> {
    let _lock = env_lock();
    let file = config_home(concat!(
        "[cmds.install]\n",
        "dry_run = true\n",
        "full = true\n",
    ))?;
    let _env = EnvGuard::set(&ISOLATING);
    let _home = EnvGuard::set(&[("HOME", file.home.as_str())]);

    let args = install_of(load_command_from(["dbar", "install"]).expect("install parses"));
    // A bare clap flag has no "absent" representation: `--dry-run` and `--full`
    // materialize `false` when omitted, and that `false` is indistinguishable
    // from one the user typed, so the command-line layer always shadows the
    // file. This is the boolean case of the deviation recorded in
    // `clap_defaults_shadow_configuration_file_values`.
    //
    // The alternative — declaring the fields `Option<bool>` so absence is
    // representable — makes clap demand a value (`--full <FULL>`), which would
    // break `dbar install --full`. Preserving the flag spelling is worth more
    // than file-sourced booleans, so the limitation is asserted here rather
    // than worked around, and is documented in the users' guide.
    assert!(!args.dry_run, "a file cannot enable a bare clap flag");
    assert!(!args.full, "a file cannot enable a bare clap flag");
    Ok(())
}
