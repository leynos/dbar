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
    /// ```rust,ignore
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
    /// ```rust,ignore
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
    /// ```rust,ignore
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
    /// ```rust,ignore
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
    /// ```rust,ignore
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
    /// ```rust,ignore
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
    /// ```rust,ignore
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
    /// ```rust,ignore
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
    /// ```rust,ignore
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
            "left" | "Left" => Ok(Self::Left),
            "right" | "Right" => Ok(Self::Right),
            _ => Err(format!("invalid status position: {value}")),
        }
    }
}
