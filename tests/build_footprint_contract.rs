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

    use cap_std::{
        ambient_authority,
        fs_utf8::{Dir, camino::Utf8PathBuf},
    };
    use rstest::rstest;

    use super::{CARGO_CONFIG, pinned_channel};

    /// Variables that would add a wrapper or override the profile under test.
    const CLEARED: [&str; 12] = [
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "CARGO_BUILD_RUSTC_WRAPPER",
        "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
        "CARGO_ENCODED_RUSTFLAGS",
        "CARGO_BUILD_RUSTFLAGS",
        "RUSTFLAGS",
        "CARGO_INCREMENTAL",
        "CARGO_BUILD_INCREMENTAL",
        "CARGO_BUILD_BUILD_DIR",
        "CARGO_PROFILE_DEV_DEBUG",
        "CARGO_PROFILE_DEV_INCREMENTAL",
    ];

    /// The fixture crate's manifest. It is its own workspace root, so a target
    /// directory inside the checkout (as under `cargo llvm-cov`) cannot make
    /// Cargo find the checkout's workspace above it.
    const FIXTURE_MANIFEST: &str =
        "[package]\nname = \"footprint\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[workspace]\n";

    /// A fixture build: its directory name and the documented overrides it applies.
    #[derive(Clone, Copy, Debug)]
    struct Fixture<'a> {
        name: &'a str,
        overrides: &'a [(&'a str, &'a str)],
    }

    /// One documented override of the footprint defaults, and what it restores.
    #[derive(Clone, Copy, Debug)]
    struct OverrideCase {
        fixture: &'static str,
        variable: &'static str,
        setting: &'static str,
        expected: &'static str,
    }

    /// Runs `cargo build -v` in a fixture directory under a toolchain channel and the given
    /// overrides, and returns what Cargo printed. A test of the parsing path passes a function
    /// that returns canned text instead, so no process runs.
    type CargoRunner = fn(&Utf8PathBuf, &str, &[(&str, &str)]) -> Result<String, String>;

    /// The process adapter: the one place that runs the real Cargo.
    fn real_cargo(
        root: &Utf8PathBuf,
        channel: &str,
        overrides: &[(&str, &str)],
    ) -> Result<String, String> {
        let mut command = Command::new("cargo");
        command
            .arg(format!("+{channel}"))
            .args(["build", "--color", "never", "--offline", "-v"])
            .current_dir(root.as_std_path())
            .env("CARGO_TARGET_DIR", root.join("target").as_std_path());
        for variable in CLEARED {
            command.env_remove(variable);
        }
        for (variable, setting) in overrides {
            command.env(variable, setting);
        }
        let output = command
            .output()
            .map_err(|error| format!("running cargo: {error}"))?;
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        if output.status.success() {
            Ok(stderr)
        } else {
            Err(format!("cargo build failed:\n{stderr}"))
        }
    }

    /// Returns the rustc command line for the fixture crate in Cargo's output.
    fn rustc_line(stderr: &str) -> Result<String, String> {
        stderr
            .lines()
            .find(|line| line.contains("Running") && line.contains("--crate-name footprint"))
            .map(str::to_owned)
            .ok_or_else(|| format!("no rustc command line in Cargo's output:\n{stderr}"))
    }

    /// Writes the fixture crate: a manifest, a library, and the repository's Cargo configuration.
    fn write_fixture(root: &Utf8PathBuf) -> Result<(), String> {
        let fail = |step: &'static str| move |error: std::io::Error| format!("{step}: {error}");
        Dir::create_ambient_dir_all(root, ambient_authority())
            .map_err(fail("creating the fixture directory"))?;
        let dir = Dir::open_ambient_dir(root, ambient_authority())
            .map_err(fail("opening the fixture directory"))?;
        dir.create_dir_all("src").map_err(fail("creating src"))?;
        dir.create_dir_all(".cargo")
            .map_err(fail("creating .cargo"))?;
        let manifest = FIXTURE_MANIFEST.to_owned();
        dir.write("Cargo.toml", manifest)
            .map_err(fail("writing Cargo.toml"))?;
        dir.write("src/lib.rs", "pub fn footprint() {}\n")
            .map_err(fail("writing the library"))?;
        dir.write(".cargo/config.toml", CARGO_CONFIG)
            .map_err(fail("writing the configuration"))
    }

    /// Builds the fixture crate in `root` with a runner and returns the rustc command line Cargo
    /// ran.
    fn build_fixture(
        runner: CargoRunner,
        root: &Utf8PathBuf,
        case: &Fixture,
    ) -> Result<String, String> {
        write_fixture(root)?;
        let channel = pinned_channel().ok_or("rust-toolchain.toml must name a channel")?;
        rustc_line(&runner(root, &channel, case.overrides)?)
    }

    /// Builds the fixture crate in a directory of its own, so two test processes cannot share
    /// one, removes it afterwards, and returns the rustc command line Cargo ran.
    fn compile_fixture(runner: CargoRunner, case: &Fixture) -> Result<String, String> {
        let parent = Utf8PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
        let name = format!("{}-{}", case.name, std::process::id());
        let built = build_fixture(runner, &parent.join(&name), case);
        // The build's result matters more than tidying up after it.
        if let Ok(directory) = Dir::open_ambient_dir(&parent, ambient_authority()) {
            directory.remove_dir_all(&name).ok();
        }
        built
    }

    /// Scenario: a default development build.
    ///
    /// Invariant: rustc gets no debug information and no incremental directory,
    /// and Cargo passes the native `embed-metadata=no` setting.
    #[test]
    fn a_default_build_has_no_debug_info_or_incremental_state() -> Result<(), String> {
        let command = compile_fixture(
            real_cargo,
            &Fixture {
                name: "footprint-default",
                overrides: &[],
            },
        )?;
        if command.contains("-C debuginfo") {
            return Err(format!("debug info must be off: {command}"));
        }
        if command.contains("-C incremental") {
            return Err(format!("incremental must be off: {command}"));
        }
        if !command.contains("-Z embed-metadata=no") {
            return Err(format!(
                "Cargo must coordinate metadata natively: {command}"
            ));
        }
        Ok(())
    }

    /// Scenario: each documented override of the footprint defaults.
    ///
    /// Invariant: `CARGO_PROFILE_DEV_DEBUG=true` restores debug information, and
    /// `CARGO_INCREMENTAL=1` restores incremental compilation.
    #[rstest]
    #[case::debug(OverrideCase {
        fixture: "footprint-debug",
        variable: "CARGO_PROFILE_DEV_DEBUG",
        setting: "true",
        expected: "-C debuginfo=2",
    })]
    #[case::incremental(OverrideCase {
        fixture: "footprint-incremental",
        variable: "CARGO_INCREMENTAL",
        setting: "1",
        expected: "-C incremental=",
    })]
    fn a_documented_override_restores_its_setting(
        #[case] case: OverrideCase,
    ) -> Result<(), String> {
        let overrides = [(case.variable, case.setting)];
        let command = compile_fixture(
            real_cargo,
            &Fixture {
                name: case.fixture,
                overrides: &overrides,
            },
        )?;
        if command.contains(case.expected) {
            Ok(())
        } else {
            Err(format!(
                "{}={} must restore the setting ({}): {command}",
                case.variable, case.setting, case.expected
            ))
        }
    }

    /// Scenario: Cargo output with and without the fixture's rustc line, and a failed build.
    ///
    /// Invariant: the line is found beside other output, its absence and a failed build are
    /// reported as errors rather than panics, and the runner is injectable.
    #[test]
    fn the_cargo_adapter_reports_a_missing_line_and_a_failed_build() -> Result<(), String> {
        let found =
            rustc_line("   Compiling footprint\n     Running `rustc --crate-name footprint`\n")?;
        if !found.contains("--crate-name footprint") {
            return Err(format!("read the wrong line: {found}"));
        }
        if rustc_line("   Compiling other\n").is_ok() {
            return Err("a missing rustc line must be an error".to_owned());
        }
        let failing: CargoRunner = |_, _, _| Err("cargo build failed".to_owned());
        let case = Fixture {
            name: "footprint-failing",
            overrides: &[],
        };
        if compile_fixture(failing, &case).is_ok() {
            return Err("a failed build must be an error".to_owned());
        }
        Ok(())
    }
}
