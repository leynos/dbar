//! Coverage for command-line parsing and environment merging.
//!
//! These cases never write a configuration file; discovery is suppressed by the
//! harness so only the argument and environment layers are in play.
use super::*;

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

    let err =
        load_command_from(argv.iter().copied()).expect_err("invalid arguments must be rejected");
    assert!(
        matches!(err, ConfigError::Cli(_)),
        "expected a CLI error, got {err:?}"
    );
}
