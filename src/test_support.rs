//! Shared test utilities for cases that must mutate process-wide state.
//!
//! # Why the environment is mutated rather than injected
//!
//! Injecting a fake environment into configuration loading is not possible with
//! `ortho_config` 0.7, and the obstruction is in the dependency rather than in
//! this crate:
//!
//! - Subcommand merging builds its environment layer with figment's
//!   `Env::prefixed(..)` provider (`ortho_config::subcommand`), which reads
//!   `std::env` directly. No public entry point — `load_and_merge`,
//!   `load_and_merge_subcommand`, or `load_and_merge_subcommand_for` — accepts
//!   an environment source, so there is nowhere to pass one.
//! - Configuration-file discovery reads `HOME`, `USERPROFILE`,
//!   `XDG_CONFIG_HOME`, and `XDG_CONFIG_DIRS` through `std::env::var_os` in
//!   `ortho_config::discovery` and `ortho_config::subcommand::paths`, so even
//!   the file layer is selected from the ambient environment.
//!
//! `mockable` does supply an `Env` trait and a `MockEnv`, and this crate already
//! depends on it for [`mockable::Clock`]. The gap is not that a fake environment
//! cannot be *expressed*; it is that `ortho_config` cannot be told to consult
//! one. Interposing would mean reimplementing `ortho_config`'s provider stack and
//! precedence rules here, which would make the tests exercise a reimplementation
//! rather than the code that actually runs.
//!
//! `dbar`'s own configuration code reads nothing from the environment:
//! `load_command_from` already takes its arguments explicitly, and
//! `load_command` reads only `std::env::args_os()`, which is arguments rather
//! than environment variables. There is therefore no seam on this side of the
//! boundary to inject at either.
//!
//! The fallback is this module: one lock and one guard shared by every
//! environment-dependent test in the crate, so mutations serialize against each
//! other and every variable is restored. Integration tests under `tests/` are
//! separate crates and cannot see this module; none of them mutates the
//! environment today, and any that needs to should promote this module to a
//! `test-support` crate rather than growing a second guard.

use std::ffi::OsString;
use std::sync::{Mutex, MutexGuard, OnceLock};

/// Serializes every test that touches process-wide environment state.
///
/// Tests that only read the environment take the lock too: a concurrent
/// mutation would otherwise perturb them.
pub fn env_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Holds the [`env_lock`] guard and restores the variables it captured.
///
/// The lock is acquired by [`EnvGuard::set`] before any variable is read or
/// mutated and released only when the guard drops, so a caller cannot lose
/// isolation by forgetting to take it. The mutex is not reentrant, so a test
/// must build exactly one guard: additional variables are layered on with
/// [`EnvGuard::and`], which reuses the lock already held.
pub struct EnvGuard {
    saved: Vec<(String, Option<OsString>)>,
    /// Held for the guard's lifetime; every mutation below relies on it.
    _lock: MutexGuard<'static, ()>,
}

impl EnvGuard {
    /// Takes the [`env_lock`], then applies `pairs`, capturing previous values.
    pub fn set(pairs: &[(&str, &str)]) -> Self {
        let lock = env_lock();
        let mut guard = Self {
            saved: Vec::new(),
            _lock: lock,
        };
        guard.apply(pairs);
        guard
    }

    /// Applies `pairs` too, under the lock this guard already holds.
    ///
    /// Calling [`EnvGuard::set`] a second time in one test would deadlock on
    /// the non-reentrant mutex, so overlapping sets chain through this instead.
    #[must_use]
    pub fn and(mut self, pairs: &[(&str, &str)]) -> Self {
        self.apply(pairs);
        self
    }

    /// Records each key's current value, then writes the requested one.
    fn apply(&mut self, pairs: &[(&str, &str)]) {
        for (key, value) in pairs {
            self.saved.push(((*key).to_owned(), std::env::var_os(key)));
            // SAFETY: the guard holds `env_lock`, which serializes every
            // mutation through this module, and `Drop` restores the previous
            // value.
            unsafe { std::env::set_var(key, value) };
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // Restored in reverse, so a key written twice (`HOME` is set by the
        // isolating set and then redirected) ends at its original value rather
        // than at the intermediate one.
        for (key, value) in self.saved.iter().rev() {
            // SAFETY: as above; the lock is held until this guard drops.
            match value {
                Some(previous) => unsafe { std::env::set_var(key, previous) },
                None => unsafe { std::env::remove_var(key) },
            }
        }
    }
}
