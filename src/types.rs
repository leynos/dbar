//! Domain-specific newtypes and shared data structures.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
/// A project name derived from git metadata or directory names.
pub struct ProjectName(String);

impl ProjectName {
    /// Create a project name wrapper.
    ///
    /// # Examples
    ///
    /// ```text
    /// use dbar::types::ProjectName;
    ///
    /// let name = ProjectName::new("dbar");
    /// assert_eq!(name.as_ref(), "dbar");
    /// ```
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }
}

impl AsRef<str> for ProjectName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProjectName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
/// A git branch name wrapper.
pub struct BranchName(String);

impl BranchName {
    /// Create a branch name wrapper.
    ///
    /// # Examples
    ///
    /// ```text
    /// use dbar::types::BranchName;
    ///
    /// let branch = BranchName::new("main");
    /// assert_eq!(branch.as_ref(), "main");
    /// ```
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }
}

impl AsRef<str> for BranchName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for BranchName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Number of commits ahead of upstream.
pub struct AheadCount(u32);

impl AheadCount {
    /// Create an ahead count wrapper.
    ///
    /// # Examples
    ///
    /// ```text
    /// use dbar::types::AheadCount;
    ///
    /// let count = AheadCount::new(2);
    /// assert_eq!(count.value(), 2);
    /// ```
    pub const fn new(count: u32) -> Self {
        Self(count)
    }

    /// Return the underlying count value.
    ///
    /// # Examples
    ///
    /// ```text
    /// use dbar::types::AheadCount;
    ///
    /// let count = AheadCount::new(1);
    /// assert_eq!(count.value(), 1);
    /// ```
    pub const fn value(self) -> u32 {
        self.0
    }
}

impl fmt::Display for AheadCount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Number of commits behind upstream.
pub struct BehindCount(u32);

impl BehindCount {
    /// Create a behind count wrapper.
    ///
    /// # Examples
    ///
    /// ```text
    /// use dbar::types::BehindCount;
    ///
    /// let count = BehindCount::new(3);
    /// assert_eq!(count.value(), 3);
    /// ```
    pub const fn new(count: u32) -> Self {
        Self(count)
    }

    /// Return the underlying count value.
    ///
    /// # Examples
    ///
    /// ```text
    /// use dbar::types::BehindCount;
    ///
    /// let count = BehindCount::new(1);
    /// assert_eq!(count.value(), 1);
    /// ```
    pub const fn value(self) -> u32 {
        self.0
    }
}

impl fmt::Display for BehindCount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
/// GitHub pull request number as a string.
pub struct PrNumber(String);

impl PrNumber {
    /// Create a PR number wrapper.
    ///
    /// # Examples
    ///
    /// ```text
    /// use dbar::types::PrNumber;
    ///
    /// let pr = PrNumber::new("42");
    /// assert_eq!(pr.to_string(), "42");
    /// ```
    pub fn new(number: impl Into<String>) -> Self {
        Self(number.into())
    }
}

impl fmt::Display for PrNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
/// Cache time-to-live in seconds.
pub struct CacheTtlSeconds(u64);

impl CacheTtlSeconds {
    /// Create a TTL wrapper.
    ///
    /// # Examples
    ///
    /// ```text
    /// use dbar::types::CacheTtlSeconds;
    ///
    /// let ttl = CacheTtlSeconds::new(30);
    /// assert_eq!(ttl.value(), 30);
    /// ```
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Return the raw TTL value.
    ///
    /// # Examples
    ///
    /// ```text
    /// use dbar::types::CacheTtlSeconds;
    ///
    /// let ttl = CacheTtlSeconds::new(5);
    /// assert_eq!(ttl.value(), 5);
    /// ```
    pub const fn value(self) -> u64 {
        self.0
    }
}

impl Default for CacheTtlSeconds {
    fn default() -> Self {
        Self::new(60)
    }
}

impl fmt::Display for CacheTtlSeconds {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for CacheTtlSeconds {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let parsed = value
            .parse::<u64>()
            .map_err(|err| format!("invalid cache ttl: {err}"))?;
        Ok(Self(parsed))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
// Serde uses the same lowercase spellings as `Display` and `FromStr`, and
// `FromStr` accepts nothing else, so a configuration file, an environment
// variable, and a command-line flag all take exactly `left` or `right`.
#[serde(rename_all = "lowercase")]
/// tmux status line placement for the install snippet.
pub enum StatusPosition {
    /// Apply the snippet to `status-left`.
    #[default]
    Left,
    /// Apply the snippet to `status-right`.
    Right,
}

impl fmt::Display for StatusPosition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Left => f.write_str("left"),
            Self::Right => f.write_str("right"),
        }
    }
}

impl FromStr for StatusPosition {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "left" => Ok(Self::Left),
            "right" => Ok(Self::Right),
            _ => Err(format!("invalid status position: {value}")),
        }
    }
}

#[cfg(test)]
mod tests {
    //! Tests for TTL and status-position parsing, defaults, and display.
    use super::*;
    use rstest::rstest;

    #[rstest]
    fn cache_ttl_default_is_sixty_seconds() {
        assert_eq!(CacheTtlSeconds::default().value(), 60);
    }

    #[rstest]
    #[case("0", 0)]
    #[case("30", 30)]
    #[case("18446744073709551615", u64::MAX)]
    fn cache_ttl_parses_valid_values(#[case] input: &str, #[case] expected: u64) {
        let ttl: CacheTtlSeconds = input.parse().expect("valid ttl");
        assert_eq!(ttl.value(), expected);
    }

    #[rstest]
    #[case::empty("")]
    #[case::not_a_number("abc")]
    #[case::negative("-1")]
    #[case::fractional("1.5")]
    #[case::overflow("18446744073709551616")]
    fn cache_ttl_rejects_invalid_values(#[case] input: &str) {
        let err = input
            .parse::<CacheTtlSeconds>()
            .expect_err("invalid ttl must be rejected");
        assert!(err.starts_with("invalid cache ttl"), "unexpected: {err}");
    }

    #[rstest]
    fn cache_ttl_round_trips_through_display() {
        let ttl = CacheTtlSeconds::new(45);
        assert_eq!(ttl.to_string(), "45");
        assert_eq!(
            ttl.to_string().parse::<CacheTtlSeconds>().expect("reparse"),
            ttl
        );
    }

    #[rstest]
    #[case("left", StatusPosition::Left)]
    #[case("right", StatusPosition::Right)]
    fn status_position_parses_valid_values(#[case] input: &str, #[case] expected: StatusPosition) {
        assert_eq!(input.parse::<StatusPosition>().expect("valid"), expected);
    }

    #[rstest]
    #[case::empty("")]
    #[case::unknown("middle")]
    #[case::upper("LEFT")]
    // Serde rejects the capitalized spellings, so `FromStr` must too; see
    // `status_position_rejects_capitalized_serde_values`.
    #[case::capitalized_left("Left")]
    #[case::capitalized_right("Right")]
    #[case::padded(" left")]
    fn status_position_rejects_invalid_values(#[case] input: &str) {
        let err = input
            .parse::<StatusPosition>()
            .expect_err("invalid position must be rejected");
        assert!(
            err.starts_with("invalid status position"),
            "unexpected: {err}"
        );
    }

    #[rstest]
    #[case(StatusPosition::Left, "left")]
    #[case(StatusPosition::Right, "right")]
    fn status_position_displays_lowercase(
        #[case] position: StatusPosition,
        #[case] expected: &str,
    ) {
        assert_eq!(position.to_string(), expected);
        // Display output must be re-parseable, so the two stay in step.
        assert_eq!(
            expected.parse::<StatusPosition>().expect("reparse"),
            position
        );
    }

    #[rstest]
    #[case(StatusPosition::Left, "\"left\"")]
    #[case(StatusPosition::Right, "\"right\"")]
    fn status_position_serializes_in_lowercase(
        #[case] position: StatusPosition,
        #[case] expected_json: &str,
    ) {
        // Serde must accept the same spellings as the CLI flag and `Display`,
        // so a configuration file and a command line agree.
        let json = serde_json::to_string(&position).expect("serialize");
        assert_eq!(json, expected_json);
        let parsed: StatusPosition = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, position);
        assert_eq!(json.trim_matches('"'), position.to_string());
    }

    #[rstest]
    fn status_position_rejects_capitalized_serde_values() {
        assert!(serde_json::from_str::<StatusPosition>("\"Left\"").is_err());
        assert!(serde_json::from_str::<StatusPosition>("\"Right\"").is_err());
        // The command-line and file layers agree on the rejection.
        assert!("Left".parse::<StatusPosition>().is_err());
        assert!("Right".parse::<StatusPosition>().is_err());
    }

    #[rstest]
    fn status_position_defaults_to_left() {
        assert_eq!(StatusPosition::default(), StatusPosition::Left);
    }
}
