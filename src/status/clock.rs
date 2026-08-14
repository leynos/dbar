//! Clock label rendering for the status line.
//!
//! The clock is the one status-line input that is *not* subject to the
//! fallback policy: a `clock_format` that chrono cannot render is a
//! configuration error, not a degraded probe, so it fails the command with an
//! actionable message instead of quietly rendering nothing.

use std::fmt::Write as _;
use std::io::{self, ErrorKind};

use mockable::Clock;

use crate::config::StatusArgs;
use crate::error::DbarError;

/// Render the clock label, if the clock segment is enabled.
///
/// # Examples
///
/// ```rust,ignore
/// use dbar::config::StatusArgs;
/// use dbar::status::clock::render_clock;
/// use mockable::DefaultClock;
///
/// let args = StatusArgs::default();
/// assert_eq!(render_clock(&args, &DefaultClock)?, None);
/// # Ok::<(), dbar::DbarError>(())
/// ```
///
/// # Errors
///
/// Returns an error naming the offending `clock_format` when chrono cannot
/// render it.
pub fn render_clock(args: &StatusArgs, clock: &dyn Clock) -> Result<Option<String>, DbarError> {
    if !args.show_clock.unwrap_or(false) {
        return Ok(None);
    }
    // `DelayedFormat::fmt` returns an error for invalid strftime directives, so
    // render via `write!` and surface that as a typed error rather than letting
    // `ToString::to_string` panic on a user-supplied `clock_format`.
    let mut label = String::new();
    write!(label, "{}", clock.local().format(&args.clock_format)).map_err(|_| {
        io::Error::new(
            ErrorKind::InvalidInput,
            // Naming the offending value makes the failure actionable; no
            // other configuration is disclosed.
            format!("invalid clock_format {:?}", args.clock_format),
        )
    })?;
    Ok(Some(label))
}

#[cfg(test)]
mod tests {
    //! Tests for clock rendering, including the actionable error raised for
    //! an invalid `clock_format`.

    use super::*;
    use mockable::DefaultClock;
    use rstest::rstest;

    #[rstest]
    #[case::dangling_percent("%")]
    #[case::unknown_directive("%Q")]
    fn invalid_clock_format_returns_an_actionable_error(#[case] clock_format: &str) {
        let args = StatusArgs {
            show_clock: Some(true),
            clock_format: clock_format.to_owned(),
            ..StatusArgs::default()
        };
        let clock = DefaultClock;
        // Must surface a typed error rather than panicking inside `to_string` ...
        let error = render_clock(&args, &clock).expect_err("invalid format must fail");
        // ... and name the offending value so the operator can fix it.
        assert!(error.to_string().contains(clock_format));
    }

    #[rstest]
    fn valid_clock_format_renders() {
        let args = StatusArgs {
            show_clock: Some(true),
            clock_format: "%H:%M".to_owned(),
            ..StatusArgs::default()
        };
        let clock = DefaultClock;
        let label = render_clock(&args, &clock).expect("valid format renders");
        assert!(label.is_some_and(|value| value.contains(':')));
    }

    #[rstest]
    fn clock_is_absent_when_disabled() {
        let args = StatusArgs::default();
        let clock = DefaultClock;
        assert_eq!(render_clock(&args, &clock).expect("no clock"), None);
    }
}
