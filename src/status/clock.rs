//! Clock label rendering for the status line.
//!
//! The clock is the one status-line input that is *not* subject to the
//! fallback policy: a `clock_format` that chrono cannot render is a
//! configuration error, not a degraded probe, so it fails the command with an
//! actionable message instead of quietly rendering nothing.

use std::fmt::Write as _;

use mockable::Clock;

use crate::config::{ConfigError, StatusArgs};
use crate::error::DbarError;

/// Render the clock label, if the clock segment is enabled.
///
/// # Examples
///
/// ```text
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
    // The merged arguments leave `clock_format` absent when no layer set it,
    // so the documented default is applied after merging rather than by clap.
    let format = args.clock_format_or_default();
    let mut label = String::new();
    // An unrenderable format is a configuration fault, so it is reported as
    // one; `ConfigError::InvalidClockFormat` names the offending value.
    write!(label, "{}", clock.local().format(format))
        .map_err(|_| ConfigError::InvalidClockFormat(format.to_owned()))?;
    Ok(Some(label))
}

#[cfg(test)]
mod tests {
    //! Tests for clock rendering, including the actionable error raised for
    //! an invalid `clock_format`.

    use super::*;
    use mockable::{DefaultClock, MockClock};
    use rstest::rstest;

    /// A fixed instant carrying an explicit offset, so the parse is unambiguous
    /// wherever the suite runs.
    ///
    /// Parsed rather than constructed, so the test needs no direct dependency
    /// on chrono: the target type is inferred from [`Clock::local`]'s return
    /// type by way of the closure handed to `returning`.
    const FIXED_INSTANT: &str = "2026-08-15T13:45:30+00:00";

    /// The hour and minute of an instant, taken from its `NaiveTime` display
    /// rather than from strftime.
    ///
    /// Deriving the expected label through a different formatter is what keeps
    /// the assertion below from merely re-running the code under test.
    fn hour_and_minute(rendered_time: &str) -> String {
        rendered_time.chars().take(5).collect()
    }

    #[rstest]
    #[case::dangling_percent("%")]
    #[case::unknown_directive("%Q")]
    fn invalid_clock_format_returns_an_actionable_error(#[case] clock_format: &str) {
        let args = StatusArgs {
            show_clock: Some(true),
            clock_format: Some(clock_format.to_owned()),
            ..StatusArgs::default()
        };
        let clock = DefaultClock;
        // Must surface a typed error rather than panicking inside `to_string` ...
        let error = render_clock(&args, &clock).expect_err("invalid format must fail");
        // ... and name the offending value so the operator can fix it.
        assert!(error.to_string().contains(clock_format));
    }

    #[rstest]
    fn the_label_is_the_injected_clock_rendered_through_the_configured_format() {
        let fixed = FIXED_INSTANT.parse().expect("fixed instant parses");
        let mut clock = MockClock::new();
        // `times(1)` is what rules out a second time source: a `render_clock`
        // that reached for `Local::now()` would leave the injected clock
        // unconsulted and fail the expectation. `returning` rather than
        // `return_const`, because the closure's return type is what pins
        // `fixed` to the trait's `DateTime<Local>`.
        clock.expect_local().times(1).returning(move || fixed);

        // Literal text either side of the directives, so a `render_clock` that
        // ignored `clock_format` and hardcoded one could not produce this
        // label, and so the assertion is an exact equality rather than a check
        // that the output merely contains a colon.
        let args = StatusArgs {
            show_clock: Some(true),
            clock_format: Some("at %H:%M sharp".to_owned()),
            ..StatusArgs::default()
        };

        let label = render_clock(&args, &clock).expect("valid format renders");

        // The local offset is applied to both sides alike, so this holds in
        // any timezone the suite runs in.
        let expected = format!("at {} sharp", hour_and_minute(&fixed.time().to_string()));
        assert_eq!(label.as_deref(), Some(expected.as_str()));
    }

    #[rstest]
    fn clock_is_absent_when_disabled() {
        let args = StatusArgs::default();
        let clock = DefaultClock;
        assert_eq!(render_clock(&args, &clock).expect("no clock"), None);
    }
}
