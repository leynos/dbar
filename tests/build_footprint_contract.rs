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
