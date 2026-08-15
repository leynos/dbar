//! Regression coverage for the four fields that once carried a clap default.
//!
//! `clock_format`, `pr_cache_ttl_seconds`, `dry_run`, and `full` used to
//! materialize a command-line value even when their flag was absent, so the
//! command-line layer shadowed the environment and the configuration file.
//! Each case here sets a *different* value at every layer and names the winner,
//! so none of them can pass by coincidence.
use super::*;

/// A `[cmds.status]` section for both formerly-shadowed status fields.
const STATUS_FILE: &str = concat!(
    "[cmds.status]\n",
    "clock_format = \"%d %b\"\n",
    "pr_cache_ttl_seconds = 11\n",
);

/// A `[cmds.install]` section enabling both flags.
///
/// The documented default for each is `false`, so `true` here is what
/// distinguishes a file that won from one that was ignored.
const INSTALL_FILE: &str = concat!("[cmds.install]\n", "dry_run = true\n", "full = true\n",);

#[rstest]
#[expect(
    clippy::panic_in_result_fn,
    reason = "the test returns `Result` to propagate the fallible fixture with `?`; assertions remain the idiomatic failure mechanism"
)]
fn the_configuration_file_overrides_the_status_defaults() -> Result<(), FixtureError> {
    let file = config_home(STATUS_FILE)?;
    let _env = EnvGuard::set(&ISOLATING).and(&[("HOME", file.home.as_str())]);

    let args = status_of(load_command_from(["dbar", "status"]).expect("status parses"));
    assert_eq!(args.clock_format_or_default(), "%d %b");
    assert_eq!(args.pr_cache_ttl_or_default(), CacheTtlSeconds::new(11));
    Ok(())
}

#[rstest]
#[expect(
    clippy::panic_in_result_fn,
    reason = "the test returns `Result` to propagate the fallible fixture with `?`; assertions remain the idiomatic failure mechanism"
)]
fn the_environment_overrides_the_configuration_file_for_status_defaults() -> Result<(), FixtureError>
{
    let file = config_home(STATUS_FILE)?;
    let _env = EnvGuard::set(&ISOLATING).and(&[
        ("HOME", file.home.as_str()),
        ("DBAR_CMDS_STATUS_CLOCK_FORMAT", "%S"),
        ("DBAR_CMDS_STATUS_PR_CACHE_TTL_SECONDS", "22"),
    ]);

    // Every layer offers a different value, so only the environment's can win.
    let args = status_of(load_command_from(["dbar", "status"]).expect("status parses"));
    assert_eq!(args.clock_format_or_default(), "%S");
    assert_eq!(args.pr_cache_ttl_or_default(), CacheTtlSeconds::new(22));
    Ok(())
}

#[rstest]
#[expect(
    clippy::panic_in_result_fn,
    reason = "the test returns `Result` to propagate the fallible fixture with `?`; assertions remain the idiomatic failure mechanism"
)]
fn the_command_line_overrides_every_lower_layer_for_status_defaults() -> Result<(), FixtureError> {
    let file = config_home(STATUS_FILE)?;
    let _env = EnvGuard::set(&ISOLATING).and(&[
        ("HOME", file.home.as_str()),
        ("DBAR_CMDS_STATUS_CLOCK_FORMAT", "%S"),
        ("DBAR_CMDS_STATUS_PR_CACHE_TTL_SECONDS", "22"),
    ]);

    let args = status_of(
        load_command_from([
            "dbar",
            "status",
            "--clock-format",
            "%H",
            "--pr-cache-ttl-seconds",
            "33",
        ])
        .expect("status parses"),
    );
    assert_eq!(args.clock_format_or_default(), "%H");
    assert_eq!(args.pr_cache_ttl_or_default(), CacheTtlSeconds::new(33));
    Ok(())
}

#[rstest]
#[expect(
    clippy::panic_in_result_fn,
    reason = "the test returns `Result` to propagate the fallible fixture with `?`; assertions remain the idiomatic failure mechanism"
)]
fn the_configuration_file_enables_the_install_flags() -> Result<(), FixtureError> {
    let file = config_home(INSTALL_FILE)?;
    let _env = EnvGuard::set(&ISOLATING).and(&[("HOME", file.home.as_str())]);

    // The regression: an omitted `--dry-run` used to arrive as `false` and
    // shadow the file, so both flags stayed off however the file was written.
    let args = install_of(load_command_from(["dbar", "install"]).expect("install parses"));
    assert!(args.is_dry_run(), "the file must be able to enable dry_run");
    assert!(args.is_full(), "the file must be able to enable full");
    Ok(())
}

#[rstest]
#[expect(
    clippy::panic_in_result_fn,
    reason = "the test returns `Result` to propagate the fallible fixture with `?`; assertions remain the idiomatic failure mechanism"
)]
fn the_environment_overrides_the_configuration_file_for_install_flags() -> Result<(), FixtureError>
{
    let file = config_home(INSTALL_FILE)?;
    let _env = EnvGuard::set(&ISOLATING).and(&[
        ("HOME", file.home.as_str()),
        ("DBAR_CMDS_INSTALL_DRY_RUN", "false"),
        ("DBAR_CMDS_INSTALL_FULL", "false"),
    ]);

    // The file says `true` for both, so `false` can only have come from the
    // environment; a merge that skipped the environment would leave them on.
    let args = install_of(load_command_from(["dbar", "install"]).expect("install parses"));
    assert!(!args.is_dry_run(), "the environment must override the file");
    assert!(!args.is_full(), "the environment must override the file");
    Ok(())
}

#[rstest]
#[expect(
    clippy::panic_in_result_fn,
    reason = "the test returns `Result` to propagate the fallible fixture with `?`; assertions remain the idiomatic failure mechanism"
)]
fn the_command_line_overrides_every_lower_layer_for_install_flags() -> Result<(), FixtureError> {
    // The file disables both flags and the environment leaves them alone, so a
    // flag that ends up on can only have come from the command line.
    let file = config_home(concat!(
        "[cmds.install]\n",
        "dry_run = false\n",
        "full = false\n",
    ))?;
    let _env = EnvGuard::set(&ISOLATING).and(&[
        ("HOME", file.home.as_str()),
        ("DBAR_CMDS_INSTALL_DRY_RUN", "false"),
        ("DBAR_CMDS_INSTALL_FULL", "false"),
    ]);

    let args = install_of(
        load_command_from(["dbar", "install", "--dry-run", "--full"]).expect("install parses"),
    );
    assert!(args.is_dry_run(), "the command line must win");
    assert!(args.is_full(), "the command line must win");
    Ok(())
}

#[rstest]
// `clock_format` accepts any string, so only the TTL can be invalid here.
#[case::non_numeric_ttl("DBAR_CMDS_STATUS_PR_CACHE_TTL_SECONDS", "not-a-number")]
#[case::negative_ttl("DBAR_CMDS_STATUS_PR_CACHE_TTL_SECONDS", "-1")]
fn invalid_status_environment_values_are_reported_rather_than_exiting(
    #[case] key: &str,
    #[case] value: &str,
) {
    let _env = EnvGuard::set(&ISOLATING).and(&[(key, value)]);

    let err = load_command_from(["dbar", "status"])
        .expect_err("an invalid environment value must be rejected");
    // A merge error, not a CLI error: the command line was well formed.
    assert!(
        matches!(err, ConfigError::Merge(_)),
        "expected a merge error, got {err:?}"
    );
}

#[rstest]
#[case::dry_run("DBAR_CMDS_INSTALL_DRY_RUN", "perhaps")]
#[case::full("DBAR_CMDS_INSTALL_FULL", "perhaps")]
fn invalid_install_environment_values_are_reported_rather_than_exiting(
    #[case] key: &str,
    #[case] value: &str,
) {
    let _env = EnvGuard::set(&ISOLATING).and(&[(key, value)]);

    let err = load_command_from(["dbar", "install"])
        .expect_err("an invalid environment value must be rejected");
    assert!(
        matches!(err, ConfigError::Merge(_)),
        "expected a merge error, got {err:?}"
    );
}

/// Assert that `contents` is rejected by the merge rather than by clap.
///
/// Factored out of the case list because `rstest`'s generated per-case
/// functions do not carry an `#[expect]` written on the test itself, so the
/// assertion lives in a plain function that can hold the attribute.
#[expect(
    clippy::panic_in_result_fn,
    reason = "the helper returns `Result` to propagate the fallible fixture with `?`; assertions remain the idiomatic failure mechanism"
)]
fn assert_file_value_is_a_merge_error(
    subcommand: &str,
    contents: &str,
) -> Result<(), FixtureError> {
    let file = config_home(contents)?;
    let _env = EnvGuard::set(&ISOLATING).and(&[("HOME", file.home.as_str())]);

    let err = load_command_from(["dbar", subcommand])
        .expect_err("an invalid configuration value must be rejected");
    // The file is syntactically valid TOML, so this is a typing failure inside
    // the merge rather than a parse failure, and it must still not exit.
    assert!(
        matches!(err, ConfigError::Merge(_)),
        "expected a merge error, got {err:?}"
    );
    Ok(())
}

#[rstest]
#[case::status_ttl("status", "[cmds.status]\npr_cache_ttl_seconds = \"not-a-number\"\n")]
#[case::install_dry_run("install", "[cmds.install]\ndry_run = \"perhaps\"\n")]
#[case::install_full("install", "[cmds.install]\nfull = 7\n")]
fn invalid_configuration_file_values_are_reported_rather_than_exiting(
    #[case] subcommand: &str,
    #[case] contents: &str,
) -> Result<(), FixtureError> {
    assert_file_value_is_a_merge_error(subcommand, contents)
}
