//! Coverage for command-line parsing and environment merging.
//!
//! These cases never write a configuration file; discovery is suppressed by the
//! harness so only the argument and environment layers are in play.
use super::*;

#[rstest]
fn documented_defaults_apply_when_nothing_overrides_them() {
    let _env = EnvGuard::set(&ISOLATING);

    let args = status_of(load_command_from(["dbar", "status"]).expect("bare status parses"));
    // Nothing was supplied, so every field stays absent and the documented
    // defaults come from the accessors rather than from clap. An absent field
    // is what lets a lower layer decide, so both halves are asserted.
    assert_eq!(args.clock_format, None);
    assert_eq!(args.pr_cache_ttl_seconds, None);
    assert_eq!(args.clock_format_or_default(), "%H:%M");
    assert_eq!(args.pr_cache_ttl_or_default(), CacheTtlSeconds::default());
    assert_eq!(args.show_pr, None);

    let install = install_of(load_command_from(["dbar", "install"]).expect("bare install parses"));
    assert_eq!(install.dry_run, None);
    assert_eq!(install.full, None);
    assert!(!install.is_dry_run());
    assert!(!install.is_full());
}

#[rstest]
fn command_line_values_are_applied() {
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
    assert_eq!(args.clock_format.as_deref(), Some("%H"));
    assert_eq!(
        args.pr_cache_ttl_seconds.map(CacheTtlSeconds::value),
        Some(5)
    );
    assert_eq!(args.show_pr, Some(false));
}

#[rstest]
fn install_arguments_are_applied() {
    let _env = EnvGuard::set(&ISOLATING);

    let command = load_command_from(["dbar", "install", "--position", "right", "--full"])
        .expect("install parses");
    let DbarCommand::Install(args) = command else {
        panic!("expected the install subcommand");
    };
    assert_eq!(args.position, Some(StatusPosition::Right));
    assert_eq!(args.full, Some(true));
    // The flag that was *not* typed must stay absent rather than arriving as
    // `Some(false)`, which is what would shadow the lower layers.
    assert_eq!(args.dry_run, None);
}

#[rstest]
fn environment_values_apply_when_the_command_line_is_silent() {
    let _env = EnvGuard::set(&ISOLATING).and(&[("DBAR_CMDS_STATUS_SESSION", "from-env")]);

    let args = status_of(load_command_from(["dbar", "status"]).expect("status parses"));
    assert_eq!(args.session.as_deref(), Some("from-env"));
}

#[rstest]
fn the_command_line_overrides_the_environment() {
    let _env = EnvGuard::set(&ISOLATING).and(&[("DBAR_CMDS_STATUS_SESSION", "from-env")]);

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
    let _env = EnvGuard::set(&ISOLATING);

    let err =
        load_command_from(argv.iter().copied()).expect_err("invalid arguments must be rejected");
    assert!(
        matches!(err, ConfigError::Cli(_)),
        "expected a CLI error, got {err:?}"
    );
}
