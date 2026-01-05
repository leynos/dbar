//! Error types shared across the dbar application.

use thiserror::Error;

#[derive(Debug, Error)]
/// Top-level errors returned by the dbar CLI.
pub enum DbarError {
    /// Cache access failed.
    #[error(transparent)]
    Cache(#[from] crate::cache::CacheError),
    /// Configuration loading failed.
    #[error(transparent)]
    Config(#[from] std::sync::Arc<ortho_config::OrthoError>),
    /// tmux install operations failed.
    #[error(transparent)]
    Install(#[from] crate::install::InstallError),
    /// IO errors surfaced from lower-level helpers.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
