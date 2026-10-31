//! Contract test for the development build footprint rule.
//!
//! The Concordat `rust-dev-build-footprint` rule needs the default development
//! profile (here in `.cargo/config.toml`) to disable incremental compilation and
//! debug information, and a dated nightly pin from 2026-07-31 to set the native
//! Cargo metadata key. This contract holds all three settings and refuses the raw
//! rustc metadata flags that broke build scripts.

/// Cargo configuration holding the development profile and the native metadata key.
const CARGO_CONFIG: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/.cargo/config.toml"));
/// Toolchain file whose dated nightly decides which metadata key is right.
const TOOLCHAIN: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/rust-toolchain.toml"));
/// First dated nightly that accepts the native `embed-metadata` key.
const NATIVE_KEY_FROM: &str = "2026-07-31";

/// Returns the non-comment lines of the named top-level TOML table.
fn table_lines<'a>(document: &'a str, table: &str) -> Vec<&'a str> {
    let header = format!("[{table}]");
    document
        .lines()
        .map(|line| line.split('#').next().unwrap_or("").trim())
        .skip_while(|line| *line != header)
        .skip(1)
        .take_while(|line| !line.starts_with('['))
        .filter(|line| !line.is_empty())
        .collect()
}

/// Returns the value assigned to `key` in a table's lines.
fn value<'a>(lines: &[&'a str], key: &str) -> Option<&'a str> {
    lines.iter().find_map(|line| {
        let (name, assigned) = line.split_once('=')?;
        (name.trim() == key).then_some(assigned.trim())
    })
}

/// Scenario: the default development profile.
///
/// Invariant: it keeps no incremental state and no debug information.
#[test]
fn the_development_profile_keeps_the_build_footprint_settings() {
    let profile = table_lines(CARGO_CONFIG, "profile.dev");
    assert_eq!(
        value(&profile, "incremental"),
        Some("false"),
        "profile.dev must set incremental = false"
    );
    assert!(
        matches!(value(&profile, "debug"), Some("0" | "false")),
        "profile.dev must set debug = 0 or false, found {:?}",
        value(&profile, "debug")
    );
}

/// Scenario: the Cargo configuration of a pin from 2026-07-31.
///
/// Invariant: it sets the native `embed-metadata = false` key, not the older
/// spelling.
#[test]
fn the_cargo_configuration_disables_metadata_embedding_natively() {
    let unstable = table_lines(CARGO_CONFIG, "unstable");
    assert_eq!(
        value(&unstable, "embed-metadata"),
        Some("false"),
        "[unstable] must set embed-metadata = false"
    );
    assert_eq!(
        value(&unstable, "no-embed-metadata"),
        None,
        "the older no-embed-metadata spelling is wrong for a pin from {NATIVE_KEY_FROM}"
    );
}

/// Scenario: every `rustflags` source in the Cargo configuration.
///
/// Invariant: none carries a raw rustc metadata flag.
#[test]
fn no_rustflags_source_carries_a_raw_metadata_flag() {
    let flags: Vec<&str> = CARGO_CONFIG
        .lines()
        .map(|line| line.split('#').next().unwrap_or("").trim())
        .filter(|line| line.starts_with("rustflags"))
        .collect();
    assert!(!flags.is_empty(), "expected at least one rustflags source");
    assert!(
        flags.iter().all(|line| !line.contains("embed-metadata")),
        "no raw embed-metadata flag may appear in rustflags: {flags:?}"
    );
}

/// Returns the pinned channel named in `rust-toolchain.toml`, if any.
fn pinned_channel() -> Option<String> {
    TOOLCHAIN.lines().find_map(|line| {
        let (name, assigned) = line.split_once('=')?;
        (name.trim() == "channel").then(|| assigned.trim().trim_matches('"').to_owned())
    })
}

/// Scenario: the pinned toolchain.
///
/// Invariant: it is a dated nightly from 2026-07-31, the range the native key
/// serves.
#[test]
fn the_pin_is_a_dated_nightly_that_accepts_the_native_key() {
    let channel = pinned_channel().unwrap_or_default();
    let date = channel.strip_prefix("nightly-").unwrap_or("");
    assert!(
        date >= NATIVE_KEY_FROM,
        "{channel:?} must be a dated nightly from {NATIVE_KEY_FROM}; older pins use \
         no-embed-metadata"
    );
}

mod cargo_boundary {
    //! Proves the settings at the Cargo boundary, on a one-file crate that
    //! carries this repository's own Cargo configuration, whose
    //! `[profile.dev]` table holds the settings.

    use std::process::Command;

    use camino::Utf8PathBuf;
    use cap_std::{ambient_authority, fs_utf8::Dir};
    use rstest::rstest;

    use super::{CARGO_CONFIG, pinned_channel};

    /// Variables that would add a wrapper or override the profile under test.
    const CLEARED: [&str; 11] = [
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "CARGO_BUILD_RUSTC_WRAPPER",
        "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
        "CARGO_ENCODED_RUSTFLAGS",
        "CARGO_BUILD_RUSTFLAGS",
        "RUSTFLAGS",
        "CARGO_INCREMENTAL",
        "CARGO_BUILD_INCREMENTAL",
        "CARGO_PROFILE_DEV_DEBUG",
        "CARGO_PROFILE_DEV_INCREMENTAL",
    ];

    /// Builds the fixture crate and returns the rustc command line Cargo ran.
    fn compile_fixture(name: &str, extra_env: &[(&str, &str)]) -> String {
        let root = Utf8PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
        Dir::create_ambient_dir_all(&root, ambient_authority())
            .expect("the fixture directory must be creatable");
        let dir = Dir::open_ambient_dir(&root, ambient_authority())
            .expect("the fixture directory must open");
        dir.create_dir_all("src").expect("src must be creatable");
        dir.create_dir_all(".cargo")
            .expect(".cargo must be creatable");
        dir.write(
            "Cargo.toml",
            "[package]\nname = \"footprint\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
        )
        .expect("the manifest must be writable");
        dir.write("src/lib.rs", "pub fn footprint() {}\n")
            .expect("the library must be writable");
        dir.write(".cargo/config.toml", CARGO_CONFIG)
            .expect("the configuration must be writable");
        let mut command = Command::new("cargo");
        command
            .arg(format!(
                "+{}",
                pinned_channel().expect("rust-toolchain.toml must name a channel")
            ))
            .args(["build", "--color", "never", "--offline", "-v"])
            .current_dir(root.as_std_path())
            .env("CARGO_TARGET_DIR", root.join("target").as_std_path());
        for variable in CLEARED {
            command.env_remove(variable);
        }
        for (variable, setting) in extra_env {
            command.env(variable, setting);
        }
        let output = command.output().expect("cargo must run");
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(output.status.success(), "cargo build failed:\n{stderr}");
        let Some(line) = stderr
            .lines()
            .find(|line| line.contains("Running") && line.contains("--crate-name footprint"))
        else {
            panic!("no rustc command line in Cargo's output:\n{stderr}");
        };
        line.to_owned()
    }

    /// Scenario: a default development build.
    ///
    /// Invariant: rustc gets no debug information and no incremental directory,
    /// and Cargo passes the native `embed-metadata=no` setting.
    #[test]
    fn a_default_build_has_no_debug_info_or_incremental_state() {
        let command = compile_fixture("footprint-default", &[]);
        assert!(
            !command.contains("-C debuginfo"),
            "debug info must be off: {command}"
        );
        assert!(
            !command.contains("-C incremental"),
            "incremental must be off: {command}"
        );
        assert!(
            command.contains("-Z embed-metadata=no"),
            "Cargo must coordinate metadata natively: {command}"
        );
    }

    /// Scenario: each documented override of the footprint defaults.
    ///
    /// Invariant: `CARGO_PROFILE_DEV_DEBUG=true` restores debug information, and
    /// `CARGO_INCREMENTAL=1` restores incremental compilation.
    #[rstest]
    #[case::debug("footprint-debug", "CARGO_PROFILE_DEV_DEBUG", "true", "-C debuginfo=2")]
    #[case::incremental("footprint-incremental", "CARGO_INCREMENTAL", "1", "-C incremental=")]
    fn a_documented_override_restores_its_setting(
        #[case] name: &str,
        #[case] variable: &str,
        #[case] setting: &str,
        #[case] expected: &str,
    ) {
        let command = compile_fixture(name, &[(variable, setting)]);
        assert!(
            command.contains(expected),
            "{variable}={setting} must restore the setting ({expected}): {command}"
        );
    }
}
