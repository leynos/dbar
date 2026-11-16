//! Configuration-layer value types.
//!
//! These mirror [`crate::types`] so that the clap and `ortho_config` layers can
//! parse and serialize a value without the domain types taking on those
//! derives; each converts into its domain counterpart with `From`.

use serde::{Deserialize, Serialize};
use std::str::FromStr;

use crate::types::{CacheTtlSeconds, StatusPosition};

/// Configuration-layer representation of a cache TTL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub struct ConfigCacheTtlSeconds(u64);

impl ConfigCacheTtlSeconds {
    /// Create a configuration-layer TTL from an already parsed value.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
}

impl FromStr for ConfigCacheTtlSeconds {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value
            .parse()
            .map(Self::new)
            .map_err(|error| format!("invalid cache ttl: {error}"))
    }
}

impl From<ConfigCacheTtlSeconds> for CacheTtlSeconds {
    fn from(value: ConfigCacheTtlSeconds) -> Self {
        Self::new(value.0)
    }
}

/// Configuration-layer representation of the tmux status placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfigStatusPosition {
    /// Write the left tmux status segment.
    Left,
    /// Write the right tmux status segment.
    Right,
}

impl FromStr for ConfigStatusPosition {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "left" => Ok(Self::Left),
            "right" => Ok(Self::Right),
            _ => Err(format!("invalid status position: {value}")),
        }
    }
}

impl From<ConfigStatusPosition> for StatusPosition {
    fn from(value: ConfigStatusPosition) -> Self {
        match value {
            ConfigStatusPosition::Left => Self::Left,
            ConfigStatusPosition::Right => Self::Right,
        }
    }
}
