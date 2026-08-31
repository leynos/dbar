//! Coverage for configuration-file loading and layer precedence.
//!
//! Each case writes a `.dbar.toml` into a temporary home directory, so the
//! developer's own configuration is never read.
use super::*;

/// A `status` section whose values differ from the defaults and every other
/// layer, so a test cannot pass by coincidence.
///
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
    let file = config_home(FILE_CONTENTS)?;
    let _env = EnvGuard::set(&ISOLATING).and(&[("HOME", file.home.as_str())]);

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
    let file = config_home(concat!(
        "[cmds.install]\n",
        "position = \"right\"\n",
        "path = \"/tmp/from-file.tmux.conf\"\n",
    ))?;
    let _env = EnvGuard::set(&ISOLATING).and(&[("HOME", file.home.as_str())]);

    // The documented defaults are `left` and the home directory's `.tmux.conf`;
    // both differ from the file's values, so the file must be what won.
    let args = install_of(load_command_from(["dbar", "install"]).expect("install parses"));
    assert_eq!(args.position.map(Into::into), Some(StatusPosition::Right));
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
    let file = config_home(FILE_CONTENTS)?;
    let _env = EnvGuard::set(&ISOLATING).and(&[
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
    let file = config_home(FILE_CONTENTS)?;
    let _env = EnvGuard::set(&ISOLATING).and(&[
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
    let file = config_home("[cmds.status\nsession = \"unterminated\n")?;
    let _env = EnvGuard::set(&ISOLATING).and(&[("HOME", file.home.as_str())]);

    let err = load_command_from(["dbar", "status"]).expect_err("a malformed file must be rejected");
    assert!(
        matches!(err, ConfigError::Merge(_)),
        "expected a merge error, got {err:?}"
    );
    Ok(())
}

#[rstest]
fn install_environment_values_apply_when_the_command_line_is_silent() {
    let _env = EnvGuard::set(&ISOLATING).and(&[
        ("DBAR_CMDS_INSTALL_POSITION", "right"),
        ("DBAR_CMDS_INSTALL_PATH", "/tmp/from-env.tmux.conf"),
    ]);

    let args = install_of(load_command_from(["dbar", "install"]).expect("install parses"));
    assert_eq!(args.position.map(Into::into), Some(StatusPosition::Right));
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
    let file = config_home(concat!(
        "[cmds.install]\n",
        "position = \"right\"\n",
        "path = \"/tmp/from-file.tmux.conf\"\n",
    ))?;
    let _env = EnvGuard::set(&ISOLATING).and(&[("HOME", file.home.as_str())]);

    let args = install_of(load_command_from(["dbar", "install"]).expect("install parses"));
    assert_eq!(args.position.map(Into::into), Some(StatusPosition::Right));
    assert_eq!(
        args.path.as_deref().map(camino::Utf8Path::as_str),
        Some("/tmp/from-file.tmux.conf")
    );
    Ok(())
}
